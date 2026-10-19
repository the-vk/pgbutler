//! OpenAI-backed [`AgentProvider`].

use futures::future::BoxFuture;
use rig::agent::AgentBuilder;
use rig::client::{AgentClientExt, ModelLister};
use rig::providers::openai::{self, OpenAIModelLister};

use super::AgentProvider;
use crate::agent::AgentError;
use crate::config::OpenAiConfig;

pub struct OpenAiProvider {
    client: openai::Client,
    model: String,
}

impl OpenAiProvider {
    pub fn new(config: &OpenAiConfig) -> Result<Self, AgentError> {
        let api_key = config
            .api_key
            .clone()
            .filter(|key| !key.trim().is_empty())
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .ok_or_else(|| {
                AgentError::Config("OpenAI api_key is not set (config or OPENAI_API_KEY)".into())
            })?;
        let client = openai::Client::builder()
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

impl AgentProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn default_model(&self) -> &str {
        &self.model
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<String>, AgentError>> {
        Box::pin(async move {
            let models = OpenAIModelLister::new(self.client.clone())
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
