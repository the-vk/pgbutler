//! Provider abstraction for the analyzer agent.

use std::sync::Arc;

use futures::future::BoxFuture;
use rig::agent::AgentBuilder;

use crate::config::{Config, ProviderConfig};

use super::AgentError;

mod ollama;

use ollama::OllamaProvider;

/// A pluggable LLM backend.
pub trait AgentProvider: Send + Sync {
    /// Short identifier, matching the `provider` configuration value.
    fn name(&self) -> &'static str;

    /// Model used when none is selected explicitly.
    fn default_model(&self) -> &str;

    /// Models offered by the backend.
    fn list_models(&self) -> BoxFuture<'_, Result<Vec<String>, AgentError>>;

    /// Start building an agent for `model`; the caller adds preamble, tools
    /// and hooks.
    fn agent_builder(&self, model: &str) -> Result<AgentBuilder, AgentError>;
}

pub fn build(config: &Config) -> Result<Arc<dyn AgentProvider>, AgentError> {
    let provider_config = config.select_provider().map_err(|v| AgentError::Config(v))?;
    match provider_config {
        ProviderConfig::Ollama(ollama_config) => {
            Ok(Arc::new(OllamaProvider::new(&ollama_config)?))
        }
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
