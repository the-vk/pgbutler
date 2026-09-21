use std::sync::Arc;

use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_postgres::Client;

use crate::agent::ToolError;

/// Arguments for `GetQueryPlanTool`.
#[derive(Debug, Deserialize, Serialize)]
pub struct QueryPlanArgs {
    pub query: String,
}

/// Tool to get the structured execution plan (EXPLAIN JSON) for a PostgreSQL query.
#[derive(Clone)]
pub struct GetQueryPlanTool {
    client: Arc<Client>,
}

impl GetQueryPlanTool {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

impl Tool for GetQueryPlanTool {
    const NAME: &'static str = "get_query_plan";
    type Args = QueryPlanArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Retrieve the detailed JSON execution plan (EXPLAIN) for a given PostgreSQL SQL query."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The PostgreSQL SQL query to explain"
                }
            },
            "required": ["query"]
        })
    }

    async fn call(
        &self,
        _ctx: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        log::info!(sql = args.query; "Running the tool 'get_query_plan'");
        let plan_json = crate::db::explain(
            &self.client,
            &args.query,
            Some(crate::db::ExplainFormat::Json),
        )
        .await?;
        
        log::debug!(execution_plan = plan_json; "Execution plan is ready.");
        log::info!("The tool 'get_query_plan' finished.");
        Ok(plan_json)
    }
}
