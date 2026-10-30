//! Per-connection local SQLite store: `~/.pgbutler/connections/<name>.sqlite`.
//!
//! Migrations in `migrations/` are embedded into the binary and applied every
//! time a store is opened, i.e. when a connection to a server is established.
//! Queries are plain SQL; there is no ORM.

use std::path::Path;

use sqlx::{
    SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

use crate::config::{self, Connection};

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
    /// Opens (creating if needed) the store of `conn` and applies pending migrations.
    pub async fn open(conn: &Connection) -> Result<Self, StoreError> {
        Self::open_at(&config::connection_store_file(&conn.name)).await
    }

    pub async fn open_at(path: &Path) -> Result<Self, StoreError> {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_migrations() {
        let dir = std::env::temp_dir().join(format!("pgbutler-store-{}", std::process::id()));
        let path = dir.join("nested").join("srv.sqlite");

        let store = Store::open_at(&path).await.unwrap();
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
