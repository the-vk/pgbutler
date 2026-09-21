use std::sync::Arc;

use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_postgres::Client;

use crate::agent::ToolError;

/// Arguments for `GetTableSchemaTool`.
#[derive(Debug, Deserialize, Serialize)]
pub struct TableSchemaArgs {
    #[serde(default = "crate::config::default_schema")]
    pub schema_name: Option<String>,
    pub table_name: String,
}
/// Tool to get column definitions and schema information for a database table.
#[derive(Clone)]
pub struct GetTableSchemaTool {
    client: Arc<Client>,
}

impl GetTableSchemaTool {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

impl Tool for GetTableSchemaTool {
    const NAME: &'static str = "get_table_schema";
    type Args = TableSchemaArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Retrieve column definitions and metadata (data types, nullability, defaults, identity) for a specific PostgreSQL table.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "schema_name": {
                    "type": "string",
                    "description": "Schema name (defaults to 'public')"
                },
                "table_name": {
                    "type": "string",
                    "description": "Table name to inspect"
                }
            },
            "required": ["table_name"]
        })
    }

    async fn call(
        &self,
        _ctx: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        log::info!(tool = "get_table_schema", schema = args.schema_name, table = args.table_name; "Running the tool 'get_table_schema'");
        let schema = args.schema_name.as_deref().unwrap_or("public");
        let columns = crate::db::get_table_schema(&self.client, schema, &args.table_name).await?;
        let output = serde_json::to_string_pretty(&columns)?;
        log::debug!(table_schema = output; "Table schema is ready");
        log::info!(tool = "get_table_schema"; "The tool 'get_table_schema' finished");
        Ok(output)
    }
}