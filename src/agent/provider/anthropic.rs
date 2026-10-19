//! Anthropic-backed [`AgentProvider`].

use futures::future::BoxFuture;
use rig::agent::AgentBuilder;
use rig::client::{AgentClientExt, ModelLister};
use rig::providers::anthropic::{self, model_listing::AnthropicModelLister};

use super::AgentProvider;
use crate::agent::AgentError;
use crate::config::AnthropicConfig;

pub struct AnthropicProvider {
    client: anthropic::Client,
    model: String,
}

impl AnthropicProvider {
    pub fn new(config: &AnthropicConfig) -> Result<Self, AgentError> {
        let api_key = config
            .api_key
            .clone()
            .filter(|key| !key.trim().is_empty())
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .ok_or_else(|| {
                AgentError::Config("Anthropic api_key is not set (config or ANTHROPIC_API_KEY)".into())
            })?;
        let client = anthropic::Client::builder()
            .api_key(api_key)
            .base_url(config.base_url.clone().unwrap())
            .build()
            .map_err(|e| AgentError::ClientInit(e.to_string()))?;
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
            let models = AnthropicModelLister::new(self.client.clone())
                .list_all()
                .await
                .map_err(|e| AgentError::ModelListing(e.to_string()))?;
            Ok(models.iter().map(|m| m.id.clone()).collect())
        })
    }

    fn agent_builder(&self, model: &str) -> Result<AgentBuilder, AgentError> {
        Ok(self.client.agent(model))
    }
}
