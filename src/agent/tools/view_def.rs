use std::sync::Arc;

use crate::db::DbConnection;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent::ToolError;

/// Arguments for `GetViewDefTool`.
#[derive(Debug, Deserialize, Serialize)]
pub struct ViewDefArgs {
    #[serde(default = "crate::config::default_schema")]
    pub schema_name: Option<String>,
    pub rel_name: String,
}

/// Tool to get view definition.
#[derive(Clone)]
pub struct GetViewDefTool {
    db: Arc<DbConnection>,
}

impl GetViewDefTool {
    pub fn new(db: Arc<DbConnection>) -> Self {
        Self { db }
    }
}

impl Tool for GetViewDefTool {
    const NAME: &'static str = "get_view_def";
    type Args = ViewDefArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Retrieve a defintion of specific database view.".to_string()
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
        log::info!(tool = "get_view_def"; "Running the tool 'get_view_def'");
        let schema = args.schema_name.as_deref().unwrap_or("public");
        let output = self.db.get_view_def(schema, &args.rel_name).await?;
        log::debug!(tool = "get_view_def", view_def = output; "The view definition is ready");
        log::info!(tool = "get_view_def"; "The tool 'get_view_def' finished");
        Ok(output)
    }
}
