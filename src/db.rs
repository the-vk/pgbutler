//! Database connectivity with configurable TLS, controlled by
//! [`crate::config::SslMode`] (mirroring libpq's `sslmode` parameter).
//!
//! Certificate material is only required when the selected mode demands it:
//! `verify-ca`/`verify-full` require a root CA to validate the server, and
//! `verify-full` additionally checks the server hostname. A client
//! certificate/key pair is always optional and, when configured, is
//! presented for mutual TLS regardless of mode (as long as TLS is in use).

use openssl::ssl::{SslFiletype, SslMethod, SslVerifyMode};
use postgres_openssl::MakeTlsConnector;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

use crate::config::Connection;

/// Tabular (or status) outcome of running a query via the simple query protocol.
#[derive(Debug, Default, Clone)]
pub struct QueryOutcome {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Status/command-complete messages (e.g. "INSERT 0 1"), in order received.
    pub statuses: Vec<String>,
}

/// Errors that can occur while establishing a database connection.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("invalid connection configuration: {0}")]
    Config(String),
    #[error("TLS setup failed: {0}")]
    Tls(#[from] openssl::error::ErrorStack),
    #[error("connection failed: {0}")]
    Connect(#[from] tokio_postgres::Error),
}

/// Connect to PostgreSQL, applying TLS according to `conn.sslmode`.
///
/// Returns the live client; the background connection task is spawned onto
/// the current tokio runtime and its errors are dropped after logging.
pub async fn connect(conn: &Connection) -> Result<Client, DbError> {
    conn.validate().map_err(DbError::Config)?;

    let mut pg_config = tokio_postgres::Config::new();
    pg_config
        .host(&conn.host)
        .port(conn.port)
        .dbname(&conn.catalog)
        .user(&conn.login);
    if !conn.password.is_empty() {
        pg_config.password(&conn.password);
    }

    if !conn.sslmode.uses_tls() {
        pg_config.ssl_mode(tokio_postgres::config::SslMode::Disable);
        let (client, connection) = pg_config.connect(NoTls).await?;
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                eprintln!("pgbutler: connection task ended: {err}");
            }
        });
        return Ok(client);
    }

    pg_config.ssl_mode(tokio_postgres::config::SslMode::Require);

    let mut builder = openssl::ssl::SslConnector::builder(SslMethod::tls())?;
    if let Some(root) = conn.sslrootcert.as_deref() {
        builder.set_ca_file(root)?;
    }
    if let (Some(cert), Some(key)) = (conn.sslcert.as_deref(), conn.sslkey.as_deref()) {
        builder.set_certificate_file(cert, SslFiletype::PEM)?;
        builder.set_private_key_file(key, SslFiletype::PEM)?;
        builder.check_private_key()?;
    }
    // `require`/`prefer` skip verification entirely; `verify-ca`/`verify-full`
    // validate the certificate chain against the root CA set above.
    builder.set_verify(if conn.sslmode.verify_peer() {
        SslVerifyMode::PEER
    } else {
        SslVerifyMode::NONE
    });

    let mut connector = MakeTlsConnector::new(builder.build());
    // Hostname verification is only meaningful (and only enabled) for
    // `verify-full`; it's applied per-connection since it lives on
    // `ConnectConfiguration`, not on the `SslConnector` builder above.
    let verify_hostname = conn.sslmode.verify_hostname();
    connector.set_callback(move |ssl, _domain| {
        ssl.set_verify_hostname(verify_hostname);
        Ok(())
    });

    let (client, connection) = pg_config.connect(connector).await?;

    tokio::spawn(async move {
        if let Err(err) = connection.await {
            eprintln!("pgbutler: connection task ended: {err}");
        }
    });

    Ok(client)
}

/// Run one or more semicolon-separated statements via the simple query
/// protocol and collect the results into a flat, display-friendly shape.
pub async fn run_query(client: &Client, sql: &str) -> Result<QueryOutcome, DbError> {
    let messages = client.simple_query(sql).await?;
    let mut outcome = QueryOutcome::default();

    for message in messages {
        match message {
            SimpleQueryMessage::Row(row) => {
                if outcome.columns.is_empty() {
                    outcome.columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                let values = (0..row.columns().len())
                    .map(|i| row.get(i).unwrap_or("").to_string())
                    .collect();
                outcome.rows.push(values);
            }
            SimpleQueryMessage::CommandComplete(n) => {
                outcome.statuses.push(format!("{n} row(s) affected"));
            }
            _ => {}
        }
    }

    Ok(outcome)
}

/// Run an `EXPLAIN` query with detailed settings against PostgreSQL and
/// return the formatted plan output as a multi-line string.
pub async fn explain(client: &Client, sql: &str) -> Result<String, DbError> {
    let query = format!(
        "explain (analyze true, verbose true, costs true, settings true, memory true, buffers true, wal true, serialize text, timing true) {sql}"
    );
    let messages = client.simple_query(&query).await?;
    let mut lines = Vec::new();

    for message in messages {
        if let SimpleQueryMessage::Row(row) = message
            && let Some(val) = row.get(0)
        {
            lines.push(val.to_string());
        }
    }

    Ok(lines.join("\n"))
}
