//! Ollama-backed [`AgentProvider`] (the default).

use futures::future::BoxFuture;
use rig::agent::AgentBuilder;
use rig::client::{AgentClientExt, ModelLister};
use rig::providers::ollama::{self, OllamaModelLister};
use serde_json::json;

use super::AgentProvider;
use crate::agent::AgentError;
use crate::config::OllamaConfig;

pub struct OllamaProvider {
    client: ollama::Client,
    num_ctx: u64,
}

impl OllamaProvider {
    pub fn new(config: &OllamaConfig) -> Result<Self, AgentError> {
        let client = ollama::Client::builder()
            .api_key(rig::client::Nothing)
            .base_url(config.base_url.clone().unwrap())
            .build()
            .map_err(|e| AgentError::ClientInit(e.to_string()))?;
        Ok(Self {
            client,
            num_ctx: config.num_ctx.unwrap(),
        })
    }
}

impl AgentProvider for OllamaProvider {
    fn name(&self) -> &'static str {
        "ollama"
    }

    fn default_model(&self) -> &str {
        ""
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
