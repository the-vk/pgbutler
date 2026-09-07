//! `sslmode` connection parameter, mirroring libpq's semantics
//! (<https://www.postgresql.org/docs/current/libpq-ssl.html>).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// How strictly TLS is required and verified for a connection.
///
/// Only `verify-ca` and `verify-full` verify the server certificate against
/// a root CA, so only those modes require `sslrootcert` to be configured.
/// Only `verify-full` additionally checks that the certificate matches the
/// server hostname. Client certificate/key (`sslcert`/`sslkey`) are always
/// optional as far as `sslmode` is concerned: if configured, they are
/// presented to the server for mutual TLS regardless of mode (as long as
/// TLS is used at all).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    /// Never use TLS.
    Disable,
    /// Try TLS, but tolerate a plaintext connection and skip verification.
    Prefer,
    /// Require TLS, but do not verify the server certificate.
    Require,
    /// Require TLS and verify the server certificate against `sslrootcert`.
    VerifyCa,
    /// Require TLS, verify the server certificate against `sslrootcert`, and
    /// verify that it matches the server hostname.
    VerifyFull,
}

impl SslMode {
    /// All supported modes, in the order libpq documents them.
    pub const ALL: [SslMode; 5] = [
        SslMode::Disable,
        SslMode::Prefer,
        SslMode::Require,
        SslMode::VerifyCa,
        SslMode::VerifyFull,
    ];

    /// Whether this mode ever attempts a TLS handshake.
    pub fn uses_tls(self) -> bool {
        self != SslMode::Disable
    }

    /// Whether this mode verifies the server certificate chain, and
    /// therefore requires `sslrootcert` to be configured.
    pub fn requires_root_cert(self) -> bool {
        matches!(self, SslMode::VerifyCa | SslMode::VerifyFull)
    }

    /// Whether the server certificate chain should be verified against the
    /// root CA (passed to the TLS library as the peer verification mode).
    pub fn verify_peer(self) -> bool {
        self.requires_root_cert()
    }

    /// Whether the server certificate's hostname should be checked against
    /// the address being connected to.
    pub fn verify_hostname(self) -> bool {
        matches!(self, SslMode::VerifyFull)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SslMode::Disable => "disable",
            SslMode::Prefer => "prefer",
            SslMode::Require => "require",
            SslMode::VerifyCa => "verify-ca",
            SslMode::VerifyFull => "verify-full",
        }
    }
}

impl Default for SslMode {
    /// Defaults to the strictest mode, preserving pgbutler's original
    /// mandatory-mutual-TLS behavior unless the user opts into a weaker mode.
    fn default() -> Self {
        SslMode::VerifyFull
    }
}

impl fmt::Display for SslMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SslMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "disable" => Ok(SslMode::Disable),
            "prefer" => Ok(SslMode::Prefer),
            "require" => Ok(SslMode::Require),
            "verify-ca" => Ok(SslMode::VerifyCa),
            "verify-full" => Ok(SslMode::VerifyFull),
            other => Err(format!(
                "invalid sslmode '{other}': expected one of disable, prefer, require, verify-ca, verify-full"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_display_and_from_str() {
        for mode in SslMode::ALL {
            let parsed: SslMode = mode.to_string().parse().unwrap();
            assert_eq!(parsed, mode);
        }
    }

    #[test]
    fn only_verify_modes_require_root_cert() {
        assert!(!SslMode::Disable.requires_root_cert());
        assert!(!SslMode::Prefer.requires_root_cert());
        assert!(!SslMode::Require.requires_root_cert());
        assert!(SslMode::VerifyCa.requires_root_cert());
        assert!(SslMode::VerifyFull.requires_root_cert());
    }

    #[test]
    fn only_verify_full_checks_hostname() {
        assert!(!SslMode::VerifyCa.verify_hostname());
        assert!(SslMode::VerifyFull.verify_hostname());
    }
}
