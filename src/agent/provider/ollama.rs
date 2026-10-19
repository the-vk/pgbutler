//! Ollama-backed [`AgentProvider`] (the default).

use futures::future::BoxFuture;
use rig::agent::AgentBuilder;
use rig::providers::ollama;
use serde_json::json;

use super::AgentProvider;
use crate::agent::AgentError;
use crate::config::OllamaConfig;

pub struct OllamaProvider {
    client: ollama::Ollama,
    num_ctx: u64,
    model: String,
}

impl OllamaProvider {
    pub fn new(config: &OllamaConfig) -> Result<Self, AgentError> {
        let client = ollama::OllamaConfig::new()
            .with_base_url(config.base_url.clone().unwrap())
            .client();
        Ok(Self {
            client,
            num_ctx: config.num_ctx.unwrap(),
            model: config.model.clone().unwrap(),
        })
    }
}

impl AgentProvider for OllamaProvider {
    fn name(&self) -> &'static str {
        "ollama"
    }

    fn default_model(&self) -> &str {
        &self.model
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<String>, AgentError>> {
        Box::pin(async move {
            let models = self
                .client
                .list_models()
                .await
                .map_err(|e| AgentError::ModelListing(e.to_string()))?;
            Ok(models.into_iter().map(|model| model.id).collect())
        })
    }

    fn is_ready(&self) -> BoxFuture<'_, bool> {
        Box::pin(async { true })
    }

    fn agent_builder(&self, model: &str) -> Result<AgentBuilder, AgentError> {
        Ok(AgentBuilder::new(self.client.native_completion(model))
            .additional_params(json!({ "num_ctx": self.num_ctx })))
    }
}
