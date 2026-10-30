use std::sync::Arc;

use crate::db::DbConnection;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent::ToolError;

/// Arguments for `GetTableIndexesTool`.
#[derive(Debug, Deserialize, Serialize)]
pub struct TableIndexesArgs {
    #[serde(default = "crate::config::default_schema")]
    pub schema_name: Option<String>,
    pub table_name: String,
}

/// Tool to discover indexes and their usage statistics for a PostgreSQL table.
#[derive(Clone)]
pub struct GetTableIndexesTool {
    db: Arc<DbConnection>,
}

impl GetTableIndexesTool {
    pub fn new(db: Arc<DbConnection>) -> Self {
        Self { db }
    }
}

impl Tool for GetTableIndexesTool {
    const NAME: &'static str = "get_table_indexes";
    type Args = TableIndexesArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "List a table's indexes, definitions, access methods, validity, size, and usage statistics."
            .to_string()
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
        let schema = args.schema_name.as_deref().unwrap_or("public");
        let indexes = self.db.get_table_indexes(schema, &args.table_name).await?;
        let output = serde_json::to_string(&indexes)?;
        log::debug!(table_indexes = output; "Table indexes are ready");
        Ok(output)
    }
}
