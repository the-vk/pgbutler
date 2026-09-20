//! Database connectivity with configurable TLS, controlled by
//! [`crate::config::SslMode`] (mirroring libpq's `sslmode` parameter).
//!
//! Certificate material is only required when the selected mode demands it:
//! `verify-ca`/`verify-full` require a root CA to validate the server, and
//! `verify-full` additionally checks the server hostname. A client
//! certificate/key pair is always optional and, when configured, is
//! presented for mutual TLS regardless of mode (as long as TLS is in use).

use std::{collections::HashMap, str::FromStr};

use openssl::ssl::{SslFiletype, SslMethod, SslVerifyMode};
use postgres_openssl::MakeTlsConnector;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use strum::{AsRefStr, Display, EnumString};
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
    #[error("JSON deserialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Unkwnown Rel Kind: {0}")]
    UnknownRelKind(String),
    #[error("Query returned no data")]
    NoData,
}

#[derive(AsRefStr, Debug, Clone, Copy, Display, PartialEq, Eq)]
#[strum(serialize_all = "lowercase")]
pub enum ExplainFormat {
    Text,
    Xml,
    Json,
    Yaml,
}

#[derive(AsRefStr, EnumString, Debug, Clone, Copy, Display, PartialEq, Eq)]
pub enum RelKind {
    #[strum(to_string = "r")]
    Table,
    #[strum(to_string = "i")]
    Index,
    #[strum(to_string = "S")]
    Sequence,
    #[strum(to_string = "t")]
    ToastTable,
    #[strum(to_string = "v")]
    View,
    #[strum(to_string = "m")]
    MaterializedView,
    #[strum(to_string = "c")]
    CompositeType,
    #[strum(to_string = "f")]
    ForeignTable,
    #[strum(to_string = "p")]
    PartitionedTable,
    #[strum(to_string = "I")]
    PartitionedIndex,
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
pub async fn explain(
    client: &Client,
    sql: &str,
    format: Option<ExplainFormat>,
) -> Result<String, DbError> {
    let explain_format = format.unwrap_or(ExplainFormat::Text);
    let query = format!(
        "explain (analyze true, verbose true, costs true, settings true, memory true, buffers true, wal true, serialize text, timing true, format {explain_format}) {sql}"
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

/// A top-level EXPLAIN result representing a statement's execution plan and metadata.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExplainStatement {
    #[serde(rename = "Plan")]
    pub plan: PlanNode,
    #[serde(rename = "Settings", default)]
    pub settings: Option<HashMap<String, Value>>,
    #[serde(rename = "Planning", default)]
    pub planning: Option<PlanningDetails>,
    #[serde(rename = "Planning Time", default)]
    pub planning_time: Option<f64>,
    #[serde(rename = "Execution Time", default)]
    pub execution_time: Option<f64>,
    #[serde(rename = "Triggers", default)]
    pub triggers: Option<Vec<Value>>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Planning stage statistics (e.g. memory usage).
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanningDetails {
    #[serde(rename = "Memory Used", default)]
    pub memory_used: Option<i64>,
    #[serde(rename = "Memory Allocated", default)]
    pub memory_allocated: Option<i64>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// A node in PostgreSQL's query execution plan tree.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanNode {
    #[serde(rename = "Node Type")]
    pub node_type: String,
    #[serde(rename = "Strategy", default)]
    pub strategy: Option<String>,
    #[serde(rename = "Partial Mode", default)]
    pub partial_mode: Option<String>,
    #[serde(rename = "Parent Relationship", default)]
    pub parent_relationship: Option<String>,
    #[serde(rename = "Parallel Aware", default)]
    pub parallel_aware: Option<bool>,
    #[serde(rename = "Async Capable", default)]
    pub async_capable: Option<bool>,
    #[serde(rename = "Relation Name", default)]
    pub relation_name: Option<String>,
    #[serde(rename = "Schema", default)]
    pub schema: Option<String>,
    #[serde(rename = "Alias", default)]
    pub alias: Option<String>,
    #[serde(rename = "Startup Cost", default)]
    pub startup_cost: Option<f64>,
    #[serde(rename = "Total Cost", default)]
    pub total_cost: Option<f64>,
    #[serde(rename = "Plan Rows", default)]
    pub plan_rows: Option<f64>,
    #[serde(rename = "Plan Width", default)]
    pub plan_width: Option<i64>,
    #[serde(rename = "Actual Startup Time", default)]
    pub actual_startup_time: Option<f64>,
    #[serde(rename = "Actual Total Time", default)]
    pub actual_total_time: Option<f64>,
    #[serde(rename = "Actual Rows", default)]
    pub actual_rows: Option<f64>,
    #[serde(rename = "Actual Loops", default)]
    pub actual_loops: Option<i64>,
    #[serde(rename = "Disabled", default)]
    pub disabled: Option<bool>,
    #[serde(rename = "Output", default)]
    pub output: Option<Vec<String>>,
    #[serde(rename = "Workers Planned", default)]
    pub workers_planned: Option<i64>,
    #[serde(rename = "Workers Launched", default)]
    pub workers_launched: Option<i64>,
    #[serde(rename = "Single Copy", default)]
    pub single_copy: Option<bool>,
    #[serde(rename = "Plans", default)]
    pub plans: Option<Vec<PlanNode>>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl PlanNode {
    /// Recursively collect all `(schema, relation_name)` pairs from this plan node and all child plans.
    pub fn collect_relations_into(&self, acc: &mut Vec<(String, String)>, default_schema: &str) {
        if let Some(rel) = &self.relation_name {
            let schema = self.schema.as_deref().unwrap_or(default_schema).to_string();
            acc.push((schema, rel.clone()));
        }
        if let Some(sub_plans) = &self.plans {
            for sub in sub_plans {
                sub.collect_relations_into(acc, default_schema);
            }
        }
    }
}

/// Query analysis data containing the structured query plan and table schemas.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct QueryAnalysisData {
    pub statements: Vec<ExplainStatement>,
    pub table_schemas: HashMap<String, Vec<TableColumn>>,
}

/// Collect query execution plan and table schema details for all tables referenced in the query.
#[allow(dead_code)]
pub async fn collect_query_analysis(
    client: &Client,
    sql: &str,
) -> Result<QueryAnalysisData, DbError> {
    let statements = explain_model(client, sql).await?;
    let mut relations = Vec::new();
    for stmt in &statements {
        stmt.plan.collect_relations_into(&mut relations, "public");
    }
    relations.sort();
    relations.dedup();

    let mut table_schemas = HashMap::new();
    for (schema, table) in relations {
        let columns = get_table_schema(client, &schema, &table).await?;
        let key = format!("{schema}.{table}");
        table_schemas.insert(key, columns);
    }

    Ok(QueryAnalysisData {
        statements,
        table_schemas,
    })
}

/// Run an `EXPLAIN` query with JSON format against PostgreSQL, parse the output,
/// and return the deserialized query plan model.
#[allow(dead_code)]
pub async fn explain_model(client: &Client, sql: &str) -> Result<Vec<ExplainStatement>, DbError> {
    let raw_json = explain(client, sql, Some(ExplainFormat::Json)).await?;
    let model = serde_json::from_str(&raw_json)?;
    Ok(model)
}

/// Detailed schema information for a single column in a PostgreSQL table.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableColumn {
    pub catalog_name: Option<String>,
    pub schema_name: String,
    pub table_name: String,
    pub ordinal_position: i32,
    pub column_name: String,
    pub data_type: String,
    pub type_details: String,
    pub is_nullable: String,
    pub column_default: Option<String>,
    pub is_identity: String,
    pub identity_generation: Option<String>,
    pub collation_name: Option<String>,
}

const TABLE_SCHEMA_QUERY: &str = r#"
SELECT
    t.table_catalog  AS catalog_name,
    t.table_schema   AS schema_name,
    t.table_name,
    c.ordinal_position,
    c.column_name,
    c.data_type,
    COALESCE(
        CASE
            WHEN c.character_maximum_length IS NOT NULL
                THEN ' (' || c.character_maximum_length || ')'
            WHEN c.numeric_precision IS NOT NULL AND c.data_type LIKE 'timestamp%'
                THEN ' (' || c.numeric_precision || ')'
            WHEN c.numeric_precision IS NOT NULL
                THEN ' (' || c.numeric_precision || ',' || COALESCE(c.numeric_scale, 0) || ')'
        END, ''
    ) AS type_details,
    c.is_nullable,
    c.column_default,
    c.is_identity,
    c.identity_generation,
    c.collation_name
FROM information_schema.tables t
JOIN information_schema.columns c
    ON c.table_catalog = t.table_catalog
   AND c.table_schema  = t.table_schema
   AND c.table_name    = t.table_name
WHERE t.table_type = 'BASE TABLE'
  AND t.table_schema NOT IN ('pg_catalog', 'information_schema')
  AND t.table_schema = $1
  AND t.table_name = $2
ORDER BY t.table_schema, t.table_name, c.ordinal_position;
"#;

/// Query detailed schema information for columns of the given table.
#[allow(dead_code)]
pub async fn get_table_schema(
    client: &Client,
    schema_name: &str,
    table_name: &str,
) -> Result<Vec<TableColumn>, DbError> {
    let rows = client
        .query(TABLE_SCHEMA_QUERY, &[&schema_name, &table_name])
        .await?;

    let columns = rows
        .into_iter()
        .map(|row| TableColumn {
            catalog_name: row.get("catalog_name"),
            schema_name: row.get("schema_name"),
            table_name: row.get("table_name"),
            ordinal_position: row.get("ordinal_position"),
            column_name: row.get("column_name"),
            data_type: row.get("data_type"),
            type_details: row.get("type_details"),
            is_nullable: row.get("is_nullable"),
            column_default: row.get("column_default"),
            is_identity: row.get("is_identity"),
            identity_generation: row.get("identity_generation"),
            collation_name: row.get("collation_name"),
        })
        .collect();

    Ok(columns)
}

const REL_KIND_QUERY: &str = r#"
SELECT n.nspname, c.relname, relkind
FROM pg_catalog.pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = $1 AND c.relname = $2;
"#;

pub async fn get_relation_kind(
    client: &Client,
    schema_name: &str,
    relation_name: &str,
) -> Result<RelKind, DbError> {
    let rows = client
        .query(REL_KIND_QUERY, &[&schema_name, &relation_name])
        .await?;

    let rel_kind = rows
        .first()
        .map(|v| {
            let ch: i8 = v.get("relkind");
            let s = (ch as u8 as char).to_string();
            (s.clone(), RelKind::from_str(&s))
        })
        .map(|v| v.1.map_err(|_| DbError::UnknownRelKind(v.0)));

    rel_kind.unwrap_or(Err(DbError::NoData))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_column_serde_roundtrip_json() {
        let col = TableColumn {
            catalog_name: Some("postgres".to_string()),
            schema_name: "public".to_string(),
            table_name: "item".to_string(),
            ordinal_position: 1,
            column_name: "id".to_string(),
            data_type: "uuid".to_string(),
            type_details: "".to_string(),
            is_nullable: "NO".to_string(),
            column_default: Some("uuidv7()".to_string()),
            is_identity: "NO".to_string(),
            identity_generation: None,
            collation_name: None,
        };

        let json = serde_json::to_string(&col).expect("serialize to json");
        let deserialized: TableColumn = serde_json::from_str(&json).expect("deserialize from json");
        assert_eq!(col, deserialized);
    }

    #[test]
    fn explain_json_serde_deserialization() {
        let json_data = r#"
[
  {
    "Plan": {
      "Node Type": "Aggregate",
      "Strategy": "Plain",
      "Partial Mode": "Finalize",
      "Parallel Aware": false,
      "Async Capable": false,
      "Startup Cost": 16453.55,
      "Total Cost": 16453.56,
      "Plan Rows": 1,
      "Plan Width": 8,
      "Disabled": false,
      "Output": ["count(*)"],
      "Plans": [
        {
          "Node Type": "Gather",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Startup Cost": 16453.33,
          "Total Cost": 16453.54,
          "Plan Rows": 2,
          "Plan Width": 8,
          "Disabled": false,
          "Output": ["(PARTIAL count(*))"],
          "Workers Planned": 2,
          "Single Copy": false,
          "Plans": [
            {
              "Node Type": "Aggregate",
              "Strategy": "Plain",
              "Partial Mode": "Partial",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 15453.33,
              "Total Cost": 15453.34,
              "Plan Rows": 1,
              "Plan Width": 8,
              "Disabled": false,
              "Output": ["PARTIAL count(*)"],
              "Plans": [
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": true,
                  "Async Capable": false,
                  "Relation Name": "item",
                  "Schema": "public",
                  "Alias": "item",
                  "Startup Cost": 0.00,
                  "Total Cost": 14411.67,
                  "Plan Rows": 416667,
                  "Plan Width": 0,
                  "Disabled": false,
                  "Output": ["id", "name", "created_ts", "modified_ts"]
                }
              ]
            }
          ]
        }
      ]
    },
    "Settings": {
    },
    "Planning": {
      "Memory Used": 14,
      "Memory Allocated": 16
    },
    "Planning Time": 0.246
  }
]
"#;

        let statements: Vec<ExplainStatement> =
            serde_json::from_str(json_data).expect("deserialize explain json");
        assert_eq!(statements.len(), 1);

        let stmt = &statements[0];
        assert_eq!(stmt.plan.node_type, "Aggregate");
        assert_eq!(stmt.plan.strategy.as_deref(), Some("Plain"));
        assert_eq!(stmt.planning_time, Some(0.246));
        assert_eq!(stmt.planning.as_ref().and_then(|p| p.memory_used), Some(14));

        let gather = &stmt.plan.plans.as_ref().unwrap()[0];
        assert_eq!(gather.node_type, "Gather");
        assert_eq!(gather.workers_planned, Some(2));

        let partial_agg = &gather.plans.as_ref().unwrap()[0];
        assert_eq!(partial_agg.node_type, "Aggregate");

        let seq_scan = &partial_agg.plans.as_ref().unwrap()[0];
        assert_eq!(seq_scan.node_type, "Seq Scan");
        assert_eq!(seq_scan.relation_name.as_deref(), Some("item"));
        assert_eq!(seq_scan.schema.as_deref(), Some("public"));
        assert_eq!(seq_scan.plan_rows, Some(416667.0));

        let mut relations = Vec::new();
        stmt.plan.collect_relations_into(&mut relations, "public");
        assert_eq!(relations, vec![("public".to_string(), "item".to_string())]);
    }
}
