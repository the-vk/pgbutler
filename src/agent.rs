//! Rig-based AI agent for PostgreSQL query analysis and optimization.

#![allow(dead_code)]

mod tools;

use std::sync::Arc;

use rig::agent::{AgentHook, HookContext, ToolResultAction, ToolResultEvent};
use rig::client::{AgentClientExt, ModelLister};
use rig::completion::Prompt;
use rig::providers::ollama::{self, OllamaModelLister};
use serde_json::json;
use tokio_postgres::Client;

use crate::agent::tools::{GetQueryPlanTool, GetRelKindTool, GetTableSchemaTool, GetViewDefTool};

/// Default Ollama base URL for local execution.
pub const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";
/// Default model used for query analysis.
pub const DEFAULT_OLLAMA_MODEL: &str = "llama3.2";
/// Rig defaults to a budget of a single model call; raise it so the agent can
/// actually invoke tools and react to their results before answering.
pub const DEFAULT_MAX_TURNS: usize = 10;
/// Default Ollama context window (`num_ctx`), in tokens. Ollama's own default
/// (32k or less, depending on version) is easily exceeded once a few schema
/// or query-plan tool results accumulate in a single analysis run, so we
/// request a larger window explicitly rather than relying on server config.
pub const DEFAULT_OLLAMA_NUM_CTX: u64 = 65536;
/// Maximum characters kept from a single tool result before it is truncated
/// for the model. Large schemas or verbose EXPLAIN plans can otherwise
/// dominate the context budget on their own.
pub const MAX_TOOL_RESULT_CHARS: usize = 8000;

/// System prompt / preamble instructing the agent on query analysis and optimization.
pub const ANALYZER_PREAMBLE: &str = r#"You are an expert PostgreSQL database performance tuning and optimization assistant.
Your task is to analyze PostgreSQL queries, diagnose performance bottlenecks, and provide actionable recommendations for optimization.
"#;

/// Hook that caps the model-visible size of tool results, so a handful of
/// large schema/plan lookups can't exhaust the context window on their own.
/// The tool's raw result (used for telemetry/other bookkeeping) is untouched.
struct TruncatingToolResultHook {
    max_chars: usize,
}

impl AgentHook for TruncatingToolResultHook {
    async fn on_tool_result(
        &self,
        _ctx: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> ToolResultAction {
        match event.presentation.as_text() {
            Some(text) if text.len() > self.max_chars => {
                let truncated: String = text.chars().take(self.max_chars).collect();
                ToolResultAction::rewrite(format!(
                    "{truncated}\n... [truncated {} of {} characters to conserve context]",
                    text.len() - truncated.len(),
                    text.len()
                ))
            }
            _ => ToolResultAction::keep(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Database error: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("Serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Failed to initialize Ollama client: {0}")]
    ClientInit(String),
    #[error("Database error during analysis: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("Agent prompt error: {0}")]
    Prompt(#[from] rig::completion::PromptError),
    #[error("Failed to list Ollama models: {0}")]
    ModelListing(String),
}

#[derive(Debug)]
pub struct Response {
    pub answer: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_requests: usize,
}

/// Query the local Ollama daemon for the set of installed models.
pub async fn list_ollama_models(base_url: Option<&str>) -> Result<Vec<String>, AgentError> {
    let url = base_url.unwrap_or(DEFAULT_OLLAMA_URL);

    let ollama_client = ollama::Client::builder()
        .api_key(rig::client::Nothing)
        .base_url(url)
        .build()
        .map_err(|e| AgentError::ClientInit(e.to_string()))?;

    let lister = OllamaModelLister::new(ollama_client);
    let models = lister
        .list_all()
        .await
        .map_err(|e| AgentError::ModelListing(e.to_string()))?;

    Ok(models.iter().map(|m| m.id.clone()).collect())
}

/// Build a configured query analyzer agent using local Ollama.
pub fn build_analyzer_agent(
    client: Arc<Client>,
    model_name: Option<&str>,
    base_url: Option<&str>,
    num_ctx: Option<u64>,
) -> Result<rig::agent::Agent, AgentError> {
    let url = base_url.unwrap_or(DEFAULT_OLLAMA_URL);
    let model = model_name.unwrap_or(DEFAULT_OLLAMA_MODEL);
    let num_ctx = num_ctx.unwrap_or(DEFAULT_OLLAMA_NUM_CTX);

    let ollama_client = ollama::Client::builder()
        .api_key(rig::client::Nothing)
        .base_url(url)
        .build()
        .map_err(|e| AgentError::ClientInit(e.to_string()))?;

    let plan_tool = GetQueryPlanTool::new(Arc::clone(&client));
    let schema_tool = GetTableSchemaTool::new(Arc::clone(&client));
    let get_rel_kind_tool = GetRelKindTool::new(Arc::clone(&client));
    let get_view_def_tool = GetViewDefTool::new(Arc::clone(&client));

    let agent = ollama_client
        .agent(model)
        .preamble(ANALYZER_PREAMBLE)
        .tool(plan_tool)
        .tool(schema_tool)
        .tool(get_rel_kind_tool)
        .tool(get_view_def_tool)
        .default_max_turns(DEFAULT_MAX_TURNS)
        .additional_params(json!({ "num_ctx": num_ctx }))
        .add_hook(TruncatingToolResultHook {
            max_chars: MAX_TOOL_RESULT_CHARS,
        })
        .build();

    Ok(agent)
}

/// Analyze a SQL query using the Rig agent with query plan and schema inspection tools.
pub async fn analyze_query(
    client: Arc<Client>,
    sql: &str,
    model_name: Option<&str>,
    base_url: Option<&str>,
) -> Result<Response, AgentError> {
    // Collect the initial execution plan to seed into the user prompt
    let plan_json = crate::db::explain(&client, sql, Some(crate::db::ExplainFormat::Json)).await?;

    let agent = build_analyzer_agent(client, model_name, base_url, None)?;

    let prompt = format!(
        "Please analyze the following SQL query and its execution plan:\n\n\
        SQL Query:\n```sql\n{sql}\n```\n\n\
        Execution Plan (JSON):\n```json\n{plan_json}\n```\n\n\
        Interrogate the query and discover names of the relations the query is reading data from. Relations may be 
        qualified with a schema name. If not, then use default schema `public`.\n
        You can also use the `get_rel_kind` tool to understand kind of the mentioned relations - table, view, etc.\n
        If you discover a view, then use the tool `get_view_def` to get view definition and repeat the relation
        discovery step again.\n
        You can use the `get_table_schema` tool to inspect table schemas and column details as needed, \
        or `get_query_plan` if you want to inspect alternative query plans.\n\
        Please provide a detailed performance analysis with root cause, recommended indexes on table relations
        or materialized views, and query rewrites. Consider view definition changes if that could help.
        
        If possible, also do estimations on potential performance improvements with the proposed changes. Consider how
        the changes would impact CPU time on executing the query, impact on I/O time spent on reading blocks from files.
        
        Provide a structured and thorough response that includes:
            - Root Cause Analysis: Explanation of why the query is slow or inefficient based on the plan operations and costs.
            - Indexing Recommendations: Concrete `CREATE INDEX` statements with explanations of why specific columns and column orderings were chosen.
            - Query Rewrites: Optimized alternative SQL query formulations (e.g., rewriting correlated subqueries, using CTEs, optimizing JOINs, pushing down filters) with explanations of why the alternative is better.
            - Additional Recommendations: PostgreSQL configuration adjustments (e.g., work_mem, random_page_cost) or maintenance tasks (e.g., VACUUM ANALYZE) if relevant.
        "
    );

    let response = agent.prompt(&prompt).extended_details().await?;
    Ok(Response {
        answer: response.output,
        input_tokens: response.usage.input_tokens,
        output_tokens: response.usage.output_tokens,
        model_requests: response.completion_calls.len(),
    })
}

#[cfg(test)]
mod tests {
    use rig::tool::Tool;

    use crate::agent::tools::TableSchemaArgs;

    use super::*;

    #[test]
    fn query_plan_tool_metadata() {
        assert_eq!(GetQueryPlanTool::NAME, "get_query_plan");
    }

    #[test]
    fn table_schema_args_deserialize() {
        let json_data = r#"{"schema_name": "public", "table_name": "users"}"#;
        let args: TableSchemaArgs = serde_json::from_str(json_data).expect("deserialize");
        assert_eq!(args.schema_name.as_deref(), Some("public"));
        assert_eq!(args.table_name, "users");

        let json_default = r#"{"table_name": "users"}"#;
        let args_default: TableSchemaArgs =
            serde_json::from_str(json_default).expect("deserialize default");
        assert_eq!(args_default.schema_name.as_deref(), Some("public"));
        assert_eq!(args_default.table_name, "users");
    }

    #[test]
    fn analyzer_preamble_content() {
        assert!(ANALYZER_PREAMBLE.contains("PostgreSQL"));
    }
}
