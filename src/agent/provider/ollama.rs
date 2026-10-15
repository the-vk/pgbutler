//! Ollama-backed [`AgentProvider`] (the default).

use futures::future::BoxFuture;
use rig::agent::AgentBuilder;
use rig::client::{AgentClientExt, ModelLister};
use rig::providers::ollama::{self, OllamaModelLister};
use serde_json::json;

use super::AgentProvider;
use crate::agent::AgentError;

/// Default Ollama base URL for local execution.
pub const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";
/// Default model used for query analysis.
pub const DEFAULT_OLLAMA_MODEL: &str = "llama3.2";
/// Default Ollama context window (`num_ctx`), in tokens. Ollama's own default
/// (32k or less, depending on version) is easily exceeded once a few schema
/// or query-plan tool results accumulate in a single analysis run, so we
/// request a larger window explicitly rather than relying on server config.
pub const DEFAULT_OLLAMA_NUM_CTX: u64 = 65536;

pub struct OllamaProvider {
    client: ollama::Client,
    num_ctx: u64,
}

impl OllamaProvider {
    pub fn new(base_url: Option<&str>, num_ctx: Option<u64>) -> Result<Self, AgentError> {
        let client = ollama::Client::builder()
            .api_key(rig::client::Nothing)
            .base_url(base_url.unwrap_or(DEFAULT_OLLAMA_URL))
            .build()
            .map_err(|e| AgentError::ClientInit(e.to_string()))?;
        Ok(Self {
            client,
            num_ctx: num_ctx.unwrap_or(DEFAULT_OLLAMA_NUM_CTX),
        })
    }
}

impl AgentProvider for OllamaProvider {
    fn name(&self) -> &'static str {
        "ollama"
    }

    fn default_model(&self) -> &str {
        DEFAULT_OLLAMA_MODEL
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<String>, AgentError>> {
        Box::pin(async move {
            let models = OllamaModelLister::new(self.client.clone())
                .list_all()
                .await
                .map_err(|e| AgentError::ModelListing(e.to_string()))?;
            Ok(models.iter().map(|m| m.id.clone()).collect())
        })
    }

    fn agent_builder(&self, model: &str) -> Result<AgentBuilder, AgentError> {
        Ok(self
            .client
            .agent(model)
            .additional_params(json!({ "num_ctx": self.num_ctx })))
    }
}
