//! Anthropic-backed [`AgentProvider`].

use futures::future::BoxFuture;
use rig::agent::AgentBuilder;
use rig::providers::anthropic;

use super::AgentProvider;
use crate::agent::AgentError;
use crate::config::AnthropicConfig;

pub struct AnthropicProvider {
    client: anthropic::Anthropic,
    model: String,
}

impl AnthropicProvider {
    pub fn new(config: &AnthropicConfig) -> Result<Self, AgentError> {
        let api_key = Some(config.api_key.clone())
            .filter(|key| !key.trim().is_empty())
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .ok_or_else(|| {
                AgentError::Config(
                    "Anthropic api_key is not set (config or ANTHROPIC_API_KEY)".into(),
                )
            })?;
        let client = anthropic::AnthropicConfig::new(api_key)
            .with_base_url(config.base_url.clone().unwrap())
            .client();
        Ok(Self {
            client,
            model: config.model.clone().unwrap(),
        })
    }
}

impl AgentProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
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

    fn agent_builder(&self, model: &str) -> Result<AgentBuilder, AgentError> {
        Ok(AgentBuilder::new(self.client.completion(model)))
    }
}
