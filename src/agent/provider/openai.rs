//! OpenAI-backed [`AgentProvider`].

use futures::future::BoxFuture;
use rig::agent::AgentBuilder;
use rig::providers::openai::{self, OpenAIConfig};

use super::AgentProvider;
use crate::agent::AgentError;
use crate::config::OpenAiConfig;

pub struct OpenAiProvider {
    client: openai::OpenAI,
    model: String,
}

impl OpenAiProvider {
    pub fn new(config: &OpenAiConfig) -> Result<Self, AgentError> {
        let api_key = Some(config.api_key.clone())
            .filter(|key| !key.trim().is_empty())
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .ok_or_else(|| {
                AgentError::Config("OpenAI api_key is not set (config or OPENAI_API_KEY)".into())
            })?;
        let client = OpenAIConfig::new(api_key)
            .with_base_url(config.base_url.clone().unwrap())
            .client();
        Ok(Self {
            client,
            model: config.model.clone().unwrap(),
        })
    }
}

impl AgentProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
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
        Ok(AgentBuilder::new(self.client.chat(model)))
    }
}
