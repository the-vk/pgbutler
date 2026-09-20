//! Rig-based AI agent for PostgreSQL query analysis and optimization.

#![allow(dead_code)]

mod tools;

use std::sync::Arc;

use rig::client::{AgentClientExt, ModelLister};
use rig::completion::Prompt;
use rig::providers::ollama::{self, OllamaModelLister};
use tokio_postgres::Client;

use crate::agent::tools::{GetQueryPlanTool, GetTableSchemaTool};

/// Default Ollama base URL for local execution.
pub const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";
/// Default model used for query analysis.
pub const DEFAULT_OLLAMA_MODEL: &str = "llama3.2";
/// Rig defaults to a budget of a single model call; raise it so the agent can
/// actually invoke tools and react to their results before answering.
pub const DEFAULT_MAX_TURNS: usize = 10;

/// System prompt / preamble instructing the agent on query analysis and optimization.
pub const ANALYZER_PREAMBLE: &str = r#"You are an expert PostgreSQL database performance tuning and optimization assistant.
Your task is to analyze PostgreSQL queries, diagnose performance bottlenecks, and provide actionable recommendations for optimization.

Workflow:
1. Examine the query execution plan to identify the most time-consuming and costly operations (e.g. Sequential Scans on large tables, expensive Nested Loops, Hash Joins, Spills to disk, Sort operations, or high startup/total costs).
2. Use the available database schema tools to inspect table definitions, columns, data types, and nullability for all involved tables.
3. Provide a structured and thorough response that includes:
   - Root Cause Analysis: Explanation of why the query is slow or inefficient based on the plan operations and costs.
   - Indexing Recommendations: Concrete `CREATE INDEX` statements with explanations of why specific columns and column orderings were chosen.
   - Query Rewrites: Optimized alternative SQL query formulations (e.g., rewriting correlated subqueries, using CTEs, optimizing JOINs, pushing down filters) with explanations of why the alternative is better.
   - Additional Recommendations: PostgreSQL configuration adjustments (e.g., work_mem, random_page_cost) or maintenance tasks (e.g., VACUUM ANALYZE) if relevant.
"#;

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
) -> Result<rig::agent::Agent, AgentError> {
    let url = base_url.unwrap_or(DEFAULT_OLLAMA_URL);
    let model = model_name.unwrap_or(DEFAULT_OLLAMA_MODEL);

    let ollama_client = ollama::Client::builder()
        .api_key(rig::client::Nothing)
        .base_url(url)
        .build()
        .map_err(|e| AgentError::ClientInit(e.to_string()))?;

    let plan_tool = GetQueryPlanTool::new(Arc::clone(&client));
    let schema_tool = GetTableSchemaTool::new(client);

    let agent = ollama_client
        .agent(model)
        .preamble(ANALYZER_PREAMBLE)
        .tool(plan_tool)
        .tool(schema_tool)
        .default_max_turns(DEFAULT_MAX_TURNS)
        .build();

    Ok(agent)
}

/// Analyze a SQL query using the Rig agent with query plan and schema inspection tools.
pub async fn analyze_query(
    client: Arc<Client>,
    sql: &str,
    model_name: Option<&str>,
    base_url: Option<&str>,
) -> Result<String, AgentError> {
    // Collect the initial execution plan to seed into the user prompt
    let plan_json = crate::db::explain(&client, sql, Some(crate::db::ExplainFormat::Json)).await?;

    let agent = build_analyzer_agent(client, model_name, base_url)?;

    let prompt = format!(
        "Please analyze the following SQL query and its execution plan:\n\n\
        SQL Query:\n```sql\n{sql}\n```\n\n\
        Execution Plan (JSON):\n```json\n{plan_json}\n```\n\n\
        You can use the `get_table_schema` tool to inspect table schemas and column details as needed, \
        or `get_query_plan` if you want to inspect alternative query plans.\n\
        Please provide a detailed performance analysis with root cause, recommended indexes, and query rewrites."
    );

    let response = agent.prompt(&prompt).await?;
    Ok(response)
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
        assert!(ANALYZER_PREAMBLE.contains("CREATE INDEX"));
    }
}
