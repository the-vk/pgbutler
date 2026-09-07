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

mod keyring;

pub use keyring::{read_master_key, write_master_key};

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
    /// Plaintext password for `login`, kept in memory only. Encrypted at
    /// rest with the `crypto` module when the connection is serialized to
    /// disk, and transparently decrypted back to plaintext when read.
    #[serde(
        default,
        serialize_with = "serialize_password",
        deserialize_with = "deserialize_password"
    )]
    pub password: String,
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
            password: String::new(),
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

/// Serialize the plaintext password field by encrypting it (AES-256-GCM via
/// [`crate::crypto`]) and base64-encoding the ciphertext for safe storage in
/// TOML. An empty password is stored as-is, without encryption, so that
/// connections without a saved password don't force creation of a master key.
fn serialize_password<S>(password: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::Error;
    if password.is_empty() {
        return serializer.serialize_str("");
    }
    let ciphertext = crate::crypto::encrypt(password.as_bytes()).map_err(S::Error::custom)?;
    serializer.serialize_str(&openssl::base64::encode_block(&ciphertext))
}

/// Deserialize the password field, decrypting the base64-encoded ciphertext
/// produced by [`serialize_password`] back to plaintext. An empty string
/// (or a missing field, via `#[serde(default)]`) deserializes to an empty
/// password.
fn deserialize_password<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    use serde::de::Error;
    let encoded = String::deserialize(deserializer)?;
    if encoded.is_empty() {
        return Ok(String::new());
    }
    let ciphertext = openssl::base64::decode_block(&encoded).map_err(D::Error::custom)?;
    let plaintext = crate::crypto::decrypt(&ciphertext).map_err(D::Error::custom)?;
    String::from_utf8(plaintext).map_err(D::Error::custom)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "touches the real macOS login Keychain (creates/reads the master key); run manually"]
    fn password_is_encrypted_at_rest_and_round_trips() {
        let mut conn = Connection::with_defaults("pw-test-connection");
        conn.password = "s3cr3t-p@ssw0rd".to_string();

        let serialized = toml::to_string_pretty(&conn).expect("serialization should succeed");
        // The plaintext password must never appear in the serialized form.
        assert!(!serialized.contains("s3cr3t-p@ssw0rd"));

        let deserialized: Connection =
            toml::from_str(&serialized).expect("deserialization should succeed");
        assert_eq!(deserialized.password, conn.password);
    }

    #[test]
    fn empty_password_round_trips_without_encryption() {
        let conn = Connection::with_defaults("no-pw-test-connection");
        assert!(conn.password.is_empty());

        let serialized = toml::to_string_pretty(&conn).expect("serialization should succeed");
        assert!(serialized.contains("password = \"\""));

        let deserialized: Connection =
            toml::from_str(&serialized).expect("deserialization should succeed");
        assert!(deserialized.password.is_empty());
    }
}
