//! Provider abstraction for the analyzer agent.

use std::sync::Arc;

use futures::future::BoxFuture;
use rig::agent::AgentBuilder;

use crate::config::ProviderConfig;

use super::AgentError;

mod anthropic;
mod ollama;
mod openai;

use anthropic::AnthropicProvider;
use ollama::OllamaProvider;
use openai::OpenAiProvider;

/// A pluggable LLM backend.
pub trait AgentProvider: Send + Sync {
    /// Short identifier, matching the `provider` configuration value.
    fn name(&self) -> &'static str;

    /// Model used when none is selected explicitly.
    fn default_model(&self) -> &str;

    /// Models offered by the backend.
    fn list_models(&self) -> BoxFuture<'_, Result<Vec<String>, AgentError>>;

    /// Whether the backend is usable; by default, listing models must succeed.
    fn is_ready(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move { self.list_models().await.is_ok() })
    }

    /// Start building an agent for `model`; the caller adds preamble, tools
    /// and hooks.
    fn agent_builder(&self, model: &str) -> Result<AgentBuilder, AgentError>;
}

pub fn build(provider_config: &ProviderConfig) -> Result<Arc<dyn AgentProvider>, AgentError> {
    match provider_config {
        ProviderConfig::Ollama(cfg) => Ok(Arc::new(OllamaProvider::new(cfg)?)),
        ProviderConfig::Anthropic(cfg) => Ok(Arc::new(AnthropicProvider::new(cfg)?)),
        ProviderConfig::OpenAi(cfg) => Ok(Arc::new(OpenAiProvider::new(cfg)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_provider() {
        assert!(toml::from_str::<ProviderConfig>("provider = \"nope\"").is_err());
    }
}
