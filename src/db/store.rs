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
}
