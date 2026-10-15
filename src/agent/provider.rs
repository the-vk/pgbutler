//! Provider abstraction for the analyzer agent.
//!
//! An [`AgentProvider`] knows how to talk to one LLM backend: it lists the
//! models available there and produces a Rig [`AgentBuilder`] for a model.
//! Everything provider-independent (preamble, tools, hooks, streaming) lives
//! in the parent module. To add a backend, implement the trait, add a
//! variant to [`ProviderConfig`] and extend [`ProviderConfig::build`].
//!
//! The active provider is chosen at runtime from `~/.pgbutler/agent.toml`:
//!
//! ```toml
//! provider = "ollama"
//! base_url = "http://localhost:11434"
//! model = "llama3.2"
//! num_ctx = 65536
//! ```

use std::fs;
use std::sync::Arc;

use futures::future::BoxFuture;
use rig::agent::AgentBuilder;
use serde::{Deserialize, Serialize};

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

/// Runtime provider selection plus its provider-specific settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "kebab-case")]
pub enum ProviderConfig {
    Ollama {
        #[serde(default)]
        base_url: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        num_ctx: Option<u64>,
    },
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::Ollama {
            base_url: None,
            model: None,
            num_ctx: None,
        }
    }
}

impl ProviderConfig {
    /// Load `agent.toml` from the app directory; a missing file yields the
    /// default (Ollama) configuration.
    pub fn load() -> Result<Self, AgentError> {
        let path = crate::config::app_dir().join("agent.toml");
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| AgentError::Config(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(AgentError::Config(format!("{}: {e}", path.display()))),
        }
    }

    /// Configured model override, if any.
    pub fn model(&self) -> Option<&str> {
        match self {
            Self::Ollama { model, .. } => model.as_deref(),
        }
    }

    /// Instantiate the configured provider.
    pub fn build(&self) -> Result<Arc<dyn AgentProvider>, AgentError> {
        match self {
            Self::Ollama {
                base_url, num_ctx, ..
            } => Ok(Arc::new(OllamaProvider::new(
                base_url.as_deref(),
                *num_ctx,
            )?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_ollama() {
        let provider = ProviderConfig::default().build().expect("build");
        assert_eq!(provider.name(), "ollama");
    }

    #[test]
    fn parses_toml() {
        let cfg: ProviderConfig =
            toml::from_str("provider = \"ollama\"\nmodel = \"qwen3\"\nnum_ctx = 4096").unwrap();
        assert_eq!(cfg.model(), Some("qwen3"));
    }

    #[test]
    fn rejects_unknown_provider() {
        assert!(toml::from_str::<ProviderConfig>("provider = \"nope\"").is_err());
    }
}
