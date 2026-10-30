//! Local SQLite store holding data tracked about a server connection.
//!
//! Migrations in `migrations/` are embedded into the binary and applied every
//! time a store is opened, i.e. when a connection to a server is established.
//! Queries are plain SQL; there is no ORM.

use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::{
    SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

use super::IndexStat;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not create store directory: {0}")]
    Io(#[from] std::io::Error),
    #[error("store database error: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("store migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

#[derive(Clone, Debug)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Opens (creating if needed) the store file at `path` and applies pending migrations.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        Self::from_options(options, 4).await
    }

    async fn from_options(
        options: SqliteConnectOptions,
        max_connections: u32,
    ) -> Result<Self, StoreError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// Time of the most recent index statistics snapshot, if any.
    pub async fn last_index_stats_at(&self) -> Result<Option<DateTime<Utc>>, StoreError> {
        // Timestamps have a fixed width, so the text maximum is the latest one.
        let latest: Option<String> =
            sqlx::query_scalar("SELECT max(collected_at) FROM index_stats")
                .fetch_one(&self.pool)
                .await?;
        Ok(latest
            .and_then(|ts| DateTime::parse_from_rfc3339(&ts).ok())
            .map(|ts| ts.with_timezone(&Utc)))
    }

    /// Appends one snapshot of index statistics, all rows stamped with the same time.
    pub async fn record_index_stats(&self, stats: &[IndexStat]) -> Result<(), StoreError> {
        let collected_at = format_ts(&Utc::now());
        let mut tx = self.pool.begin().await?;
        for s in stats {
            sqlx::query(
                "INSERT INTO index_stats (
                     collected_at, relid, indexrelid, schemaname, relname, indexrelname,
                     idx_scan, last_idx_scan, idx_tup_read, idx_tup_fetch
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .bind(&collected_at)
            .bind(i64::from(s.relid))
            .bind(i64::from(s.indexrelid))
            .bind(&s.schemaname)
            .bind(&s.relname)
            .bind(&s.indexrelname)
            .bind(s.idx_scan)
            .bind(s.last_idx_scan.as_ref().map(format_ts))
            .bind(s.idx_tup_read)
            .bind(s.idx_tup_fetch)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// Windows over which index scan velocity is averaged, as (label, hours).
pub const VELOCITY_WINDOWS: [(&str, i64); 4] = [("1h", 1), ("6h", 6), ("24h", 24), ("7d", 168)];

/// Latest recorded values of one index and its scan velocity per window.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexReportRow {
    pub schemaname: String,
    pub relname: String,
    pub indexrelname: String,
    pub idx_scan: i64,
    pub idx_tup_read: i64,
    pub idx_tup_fetch: i64,
    pub last_idx_scan: Option<String>,
    /// Average `idx_scan` increase per hour for each of [`VELOCITY_WINDOWS`];
    /// `None` when the history does not reach back far enough or the counter was reset.
    pub scans_per_hour: [Option<f64>; 4],
}

type LatestRow = (i64, String, String, String, i64, i64, i64, Option<String>);

impl Store {
    /// Report over the latest snapshot: public schema first, then schemas
    /// ascending, and within a schema the most scanned indexes first.
    pub async fn index_stats_report(&self) -> Result<Vec<IndexReportRow>, StoreError> {
        let Some(latest_at) = self.last_index_stats_at().await? else {
            return Ok(Vec::new());
        };
        let latest: Vec<LatestRow> = sqlx::query_as(
            "SELECT indexrelid, schemaname, relname, indexrelname,
                    idx_scan, idx_tup_read, idx_tup_fetch, last_idx_scan
             FROM index_stats WHERE collected_at = ?1",
        )
        .bind(format_ts(&latest_at))
        .fetch_all(&self.pool)
        .await?;

        let mut rows: Vec<(i64, IndexReportRow)> = latest
            .into_iter()
            .map(
                |(id, schemaname, relname, indexrelname, scan, read, fetch, last)| {
                    let row = IndexReportRow {
                        schemaname,
                        relname,
                        indexrelname,
                        idx_scan: scan,
                        idx_tup_read: read,
                        idx_tup_fetch: fetch,
                        last_idx_scan: last,
                        scans_per_hour: [None; 4],
                    };
                    (id, row)
                },
            )
            .collect();

        for (slot, (_, hours)) in VELOCITY_WINDOWS.iter().enumerate() {
            let cutoff = format_ts(&(latest_at - chrono::Duration::hours(*hours)));
            let base_at: Option<String> = sqlx::query_scalar(
                "SELECT max(collected_at) FROM index_stats WHERE collected_at <= ?1",
            )
            .bind(&cutoff)
            .fetch_one(&self.pool)
            .await?;
            let Some(base_at) = base_at else { continue };
            let elapsed_hours = match DateTime::parse_from_rfc3339(&base_at) {
                Ok(ts) => (latest_at - ts.with_timezone(&Utc)).num_milliseconds() as f64 / 3.6e6,
                Err(_) => continue,
            };
            if elapsed_hours <= 0.0 {
                continue;
            }
            let base: std::collections::HashMap<i64, i64> = sqlx::query_as(
                "SELECT indexrelid, idx_scan FROM index_stats WHERE collected_at = ?1",
            )
            .bind(&base_at)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .collect();
            for (id, row) in &mut rows {
                if let Some(old) = base.get(id)
                    && row.idx_scan >= *old
                {
                    row.scans_per_hour[slot] = Some((row.idx_scan - old) as f64 / elapsed_hours);
                }
            }
        }

        let mut rows: Vec<IndexReportRow> = rows.into_iter().map(|(_, row)| row).collect();
        rows.sort_by(|a, b| {
            (a.schemaname != "public")
                .cmp(&(b.schemaname != "public"))
                .then_with(|| a.schemaname.cmp(&b.schemaname))
                .then_with(|| b.idx_scan.cmp(&a.idx_scan))
                .then_with(|| a.indexrelname.cmp(&b.indexrelname))
        });
        Ok(rows)
    }
}

fn format_ts(ts: &DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn index_stats_snapshots_are_appended() {
        let dir = std::env::temp_dir().join(format!("pgbutler-store-{}", std::process::id()));
        let path = dir.join("nested").join("srv.sqlite");

        let store = Store::open(&path).await.unwrap();
        assert_eq!(store.last_index_stats_at().await.unwrap(), None);
        let stat = IndexStat {
            relid: 25212,
            indexrelid: 25228,
            schemaname: "public".into(),
            relname: "product".into(),
            indexrelname: "product_pkey".into(),
            idx_scan: 1,
            last_idx_scan: Some(Utc::now()),
            idx_tup_read: 100000,
            idx_tup_fetch: 0,
        };
        let never_used = IndexStat {
            indexrelid: 25248,
            indexrelname: "customer_pkey".into(),
            last_idx_scan: None,
            idx_scan: 0,
            idx_tup_read: 0,
            ..stat.clone()
        };
        store
            .record_index_stats(&[stat.clone(), never_used.clone()])
            .await
            .unwrap();
        store.record_index_stats(&[stat]).await.unwrap();
        let (rows, nulls, runs): (i64, i64, i64) = sqlx::query_as(
            "SELECT count(*), count(*) - count(last_idx_scan), count(DISTINCT collected_at) FROM index_stats",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!((rows, nulls, runs), (3, 1, 2));
        let last = store.last_index_stats_at().await.unwrap().unwrap();
        assert!(Utc::now() - last < chrono::Duration::minutes(1));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn report_orders_rows_and_computes_velocity() {
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .foreign_keys(true);
        let store = Store::from_options(options, 1).await.unwrap();
        let now = Utc::now();
        let rows = [
            // (hours ago, schema, index id, name, idx_scan)
            (7, "public", 1, "a_idx", 10),
            (1, "public", 1, "a_idx", 40),
            (0, "public", 1, "a_idx", 100),
            (0, "public", 2, "b_idx", 500),
            (0, "app", 3, "c_idx", 1),
            (0, "zeta", 4, "d_idx", 9),
        ];
        for (hours_ago, schema, id, name, scans) in rows {
            let at = format_ts(&(now - chrono::Duration::hours(hours_ago)));
            sqlx::query(
                "INSERT INTO index_stats (collected_at, relid, indexrelid, schemaname, relname,
                     indexrelname, idx_scan, idx_tup_read, idx_tup_fetch)
                 VALUES (?1, 0, ?2, ?3, 't', ?4, ?5, 0, 0)",
            )
            .bind(at)
            .bind(id)
            .bind(schema)
            .bind(name)
            .bind(scans)
            .execute(&store.pool)
            .await
            .unwrap();
        }
        let report = store.index_stats_report().await.unwrap();
        let order: Vec<_> = report.iter().map(|r| r.indexrelname.as_str()).collect();
        assert_eq!(order, ["b_idx", "a_idx", "c_idx", "d_idx"]);
        let a = &report[1];
        assert!((a.scans_per_hour[0].unwrap() - 60.0).abs() < 0.01);
        assert!((a.scans_per_hour[1].unwrap() - 90.0 / 7.0).abs() < 0.01);
        assert!(a.scans_per_hour[2].is_none() && a.scans_per_hour[3].is_none());
        assert!(report[0].scans_per_hour[0].is_none());
    }
}
