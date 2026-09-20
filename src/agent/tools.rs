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
        let plan_json = crate::db::explain(
            &self.client,
            &args.query,
            Some(crate::db::ExplainFormat::Json),
        )
        .await?;
        Ok(plan_json)
    }
}

/// Arguments for `GetTableSchemaTool`.
#[derive(Debug, Deserialize, Serialize)]
pub struct TableSchemaArgs {
    #[serde(default = "default_schema")]
    pub schema_name: Option<String>,
    pub table_name: String,
}

fn default_schema() -> Option<String> {
    Some("public".to_string())
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
        let schema = args.schema_name.as_deref().unwrap_or("public");
        let columns = crate::db::get_table_schema(&self.client, schema, &args.table_name).await?;
        let output = serde_json::to_string_pretty(&columns)?;
        Ok(output)
    }
}


/// Arguments for `GetRelKindTool`.
#[derive(Debug, Deserialize, Serialize)]
pub struct RelKindArgs {
    #[serde(default = "default_schema")]
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
    type Args = TableSchemaArgs;
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
        let schema = args.schema_name.as_deref().unwrap_or("public");
        let rel_kind = crate::db::get_relation_kind(&self.client, schema, &args.table_name).await?;
        let output = rel_kind.to_string();
        Ok(output)
    }
}

/// Arguments for `GetViewDefTool`.
#[derive(Debug, Deserialize, Serialize)]
pub struct ViewDefArgs {
    #[serde(default = "default_schema")]
    pub schema_name: Option<String>,
    pub rel_name: String,
}

/// Tool to get view definition.
#[derive(Clone)]
pub struct GetViewDefTool {
    client: Arc<Client>,
}

impl GetViewDefTool {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

impl Tool for GetViewDefTool {
    const NAME: &'static str = "get_view_def";
    type Args = TableSchemaArgs;
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
        let schema = args.schema_name.as_deref().unwrap_or("public");
        let output = crate::db::get_view_def(&self.client, schema, &args.table_name).await?;
        Ok(output)
    }
}
