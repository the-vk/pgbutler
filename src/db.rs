//! Database connectivity with mandatory mutual TLS.
//!
//! Every connection is established with `sslmode=verify-full` and requires
//! a client certificate/key pair in addition to the server's root CA. There
//! is no fallback to plaintext or server-only TLS.

use openssl::ssl::{SslFiletype, SslMethod, SslVerifyMode};
use postgres_openssl::MakeTlsConnector;
use tokio_postgres::{Client, SimpleQueryMessage};

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

/// Connect to PostgreSQL using verify-full TLS with mandatory client certificates.
///
/// Returns the live client; the background connection task is spawned onto
/// the current tokio runtime and its errors are dropped after logging.
pub async fn connect(conn: &Connection) -> Result<Client, DbError> {
    conn.validate().map_err(DbError::Config)?;

    let mut builder = openssl::ssl::SslConnector::builder(SslMethod::tls())?;
    builder.set_ca_file(&conn.sslrootcert)?;
    builder.set_certificate_file(&conn.sslcert, SslFiletype::PEM)?;
    builder.set_private_key_file(&conn.sslkey, SslFiletype::PEM)?;
    builder.check_private_key()?;
    // verify-full: validate the certificate chain AND the server hostname.
    builder.set_verify(SslVerifyMode::PEER);

    let connector = MakeTlsConnector::new(builder.build());

    let mut pg_config = tokio_postgres::Config::new();
    pg_config
        .host(&conn.host)
        .port(conn.port)
        .dbname(&conn.catalog)
        .user(&conn.login)
        .ssl_mode(tokio_postgres::config::SslMode::Require);
    if !conn.password.is_empty() {
        pg_config.password(&conn.password);
    }

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
