use std::sync::Arc;

use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_postgres::Client;

use crate::agent::ToolError;

/// Arguments for `GetRelKindTool`.
#[derive(Debug, Deserialize, Serialize)]
pub struct RelKindArgs {
    #[serde(default = "crate::config::default_schema")]
    pub schema_name: Option<String>,
    pub rel_name: String,
}

/// Tool to get a relation kind.
#[derive(Clone)]
pub struct GetRelKindTool {
    client: Arc<Client>,
}

impl GetRelKindTool {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

impl Tool for GetRelKindTool {
    const NAME: &'static str = "get_rel_kind";
    type Args = RelKindArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Retrieve type of a specific PostgreSQL relation (table, index, view, materialzed view, etc.).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "schema_name": {
                    "type": "string",
                    "description": "Schema name (defaults to 'public')"
                },
                "rel_name": {
                    "type": "string",
                    "description": "Relation name to inspect"
                }
            },
            "required": ["rel_name"]
        })
    }

    async fn call(
        &self,
        _ctx: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        log::info!(tool = "get_rel_kind", schema_name = args.schema_name, rel_name = args.rel_name; "Running the tool 'get_rel_kind'");
        let schema = args.schema_name.as_deref().unwrap_or("public");
        let rel_kind = crate::db::get_relation_kind(&self.client, schema, &args.rel_name).await?;
        let output = rel_kind.to_string();
        log::debug!(tool = "get_rel_kind", rel_kind = output; "Relation kind is ready");
        log::info!(tool = "get_rel_kind"; "The tool 'get_rel_kind' finished");
        Ok(output)
    }
}