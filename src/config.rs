//! Connection configuration storage.
//!
//! Connections are stored as individual TOML files under
//! `~/.pgbutler/connections/<name>.toml`. Mutual TLS (client certificate
//! verification) is mandatory: every stored connection must reference a
//! root CA certificate plus a client certificate/key pair, and `sslmode`
//! is always pinned to `verify-full`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A single PostgreSQL connection profile, persisted to disk as TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    /// Friendly, unique name for this connection (used as the file stem).
    pub name: String,
    pub host: String,
    pub port: u16,
    pub catalog: String,
    pub login: String,
    /// Path to the CA certificate used to verify the server.
    pub sslrootcert: String,
    /// Path to the client certificate presented to the server.
    pub sslcert: String,
    /// Path to the client private key matching `sslcert`.
    pub sslkey: String,
}

impl Connection {
    /// Reasonable defaults for a new connection, ready to be edited by the user.
    pub fn with_defaults(name: impl Into<String>) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let pg_dir = home.join(".postgresql");
        Self {
            name: name.into(),
            host: "localhost".to_string(),
            port: 5432,
            catalog: "postgres".to_string(),
            login: whoami_fallback(),
            sslrootcert: pg_dir.join("root.crt").to_string_lossy().into_owned(),
            sslcert: pg_dir.join("postgresql.crt").to_string_lossy().into_owned(),
            sslkey: pg_dir.join("postgresql.key").to_string_lossy().into_owned(),
        }
    }

    /// Validate that all mandatory mTLS material is configured and present on disk.
    pub fn validate(&self) -> Result<(), String> {
        if self.host.trim().is_empty() {
            return Err("host must not be empty".into());
        }
        if self.catalog.trim().is_empty() {
            return Err("database name must not be empty".into());
        }
        if self.login.trim().is_empty() {
            return Err("user must not be empty".into());
        }
        for (label, path) in [
            ("root CA certificate (sslrootcert)", &self.sslrootcert),
            ("client certificate (sslcert)", &self.sslcert),
            ("client key (sslkey)", &self.sslkey),
        ] {
            if path.trim().is_empty() {
                return Err(format!("{label} is mandatory for mutual TLS"));
            }
            if !Path::new(path).is_file() {
                return Err(format!("{label} not found at '{path}'"));
            }
        }
        Ok(())
    }
}

fn whoami_fallback() -> String {
    std::env::var("PGUSER")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "postgres".to_string())
}

/// Root directory holding connection profiles: `~/.pgbutler/connections`.
pub fn connections_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".pgbutler").join("connections")
}

/// List all connection profiles found on disk, sorted by name.
pub fn list_connections() -> std::io::Result<Vec<Connection>> {
    let dir = connections_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut connections = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let contents = fs::read_to_string(&path)?;
        match toml::from_str::<Connection>(&contents) {
            Ok(conn) => connections.push(conn),
            Err(_) => continue, // skip unreadable/malformed profiles
        }
    }
    connections.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(connections)
}

/// Persist a connection profile to `~/.pgbutler/connections/<name>.toml`.
pub fn save_connection(conn: &Connection) -> std::io::Result<PathBuf> {
    let dir = connections_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.toml", sanitize_filename(&conn.name)));
    let contents = toml::to_string_pretty(conn).expect("connection is always serializable");
    fs::write(&path, contents)?;
    Ok(path)
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
