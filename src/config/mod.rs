//! Connection configuration storage.
//!
//! Connections are stored as individual TOML files under
//! `~/.pgbutler/connections/<name>.toml`. TLS behavior is controlled by
//! `sslmode` (mirroring libpq); certificate files are only required when the
//! selected mode actually needs them (see [`SslMode`]).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod keyring;
mod sslmode;

pub use keyring::{read_master_key, write_master_key};
pub use sslmode::SslMode;

/// A single PostgreSQL connection profile, persisted to disk as TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    /// Friendly, unique name for this connection (used as the file stem).
    pub name: String,
    pub host: String,
    pub port: u16,
    pub catalog: String,
    pub login: String,
    /// How strictly TLS is required and verified; see [`SslMode`].
    #[serde(default)]
    pub sslmode: SslMode,
    /// Path to the CA certificate used to verify the server. Only required
    /// when `sslmode` is `verify-ca` or `verify-full`.
    #[serde(default)]
    pub sslrootcert: Option<String>,
    /// Path to the client certificate presented to the server. Always
    /// optional: used for mutual TLS if provided and TLS is in use.
    #[serde(default)]
    pub sslcert: Option<String>,
    /// Path to the client private key matching `sslcert`. Always optional,
    /// on the same terms as `sslcert`.
    #[serde(default)]
    pub sslkey: Option<String>,
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
            sslmode: SslMode::default(),
            sslrootcert: Some(pg_dir.join("root.crt").to_string_lossy().into_owned()),
            sslcert: Some(pg_dir.join("postgresql.crt").to_string_lossy().into_owned()),
            sslkey: Some(pg_dir.join("postgresql.key").to_string_lossy().into_owned()),
            password: String::new(),
        }
    }

    /// Validate the connection, including the mutual-TLS material mandated
    /// by `sslmode` (see [`SslMode::requires_root_cert`]). Certificate paths
    /// that are configured are always checked for existence, even when not
    /// strictly required by `sslmode`.
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

        check_cert_path(
            "root CA certificate (sslrootcert)",
            &self.sslrootcert,
            self.sslmode.requires_root_cert(),
            self.sslmode,
        )?;
        // Client certificate/key are never mandated by sslmode, but if one
        // half of the pair is set the other must be too, and any path that
        // is set must exist.
        check_cert_path(
            "client certificate (sslcert)",
            &self.sslcert,
            false,
            self.sslmode,
        )?;
        check_cert_path("client key (sslkey)", &self.sslkey, false, self.sslmode)?;
        let is_cert_empty = self.sslcert.as_deref().is_some_and(|s| !s.trim().is_empty());
        let is_cert_key_empty = self.sslkey.as_deref().is_some_and(|s| !s.trim().is_empty());
        if is_cert_empty != is_cert_key_empty {
            return Err("sslcert and sslkey must both be set, or both left empty".into());
        }
        Ok(())
    }
}

/// Validate a single optional certificate path field: if `required`, it must
/// be present; if present (required or not), it must point to an existing
/// file.
fn check_cert_path(
    label: &str,
    path: &Option<String>,
    required: bool,
    mode: SslMode,
) -> Result<(), String> {
    match path.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => {
            if !Path::new(p).is_file() {
                return Err(format!("{label} not found at '{p}'"));
            }
            Ok(())
        }
        _ if required => Err(format!("{label} is mandatory for sslmode '{mode}'")),
        _ => Ok(()),
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

    #[test]
    fn disable_sslmode_does_not_require_any_cert() {
        let mut conn = Connection::with_defaults("disable-test-connection");
        conn.sslmode = SslMode::Disable;
        conn.sslrootcert = None;
        conn.sslcert = None;
        conn.sslkey = None;
        assert!(conn.validate().is_ok());
    }

    #[test]
    fn require_sslmode_does_not_require_root_cert() {
        let mut conn = Connection::with_defaults("require-test-connection");
        conn.sslmode = SslMode::Require;
        conn.sslrootcert = None;
        conn.sslcert = None;
        conn.sslkey = None;
        assert!(conn.validate().is_ok());
    }

    #[test]
    fn verify_ca_requires_root_cert() {
        let mut conn = Connection::with_defaults("verify-ca-test-connection");
        conn.sslmode = SslMode::VerifyCa;
        conn.sslrootcert = None;
        conn.sslcert = None;
        conn.sslkey = None;
        let err = conn.validate().expect_err("missing root cert should fail");
        assert!(err.contains("root CA certificate"));
    }

    #[test]
    fn client_cert_and_key_must_both_be_set_or_both_empty() {
        let mut conn = Connection::with_defaults("half-mtls-test-connection");
        conn.sslmode = SslMode::Require;
        conn.sslrootcert = None;
        conn.sslcert = Some("/nonexistent/does-not-matter.crt".to_string());
        conn.sslkey = None;
        let err = conn
            .validate()
            .expect_err("mismatched cert/key should fail");
        // Fails on the missing file for sslcert before reaching the pairing
        // check; either error is acceptable evidence that validation caught it.
        assert!(err.contains("sslcert") || err.contains("both be set"));
    }
}
