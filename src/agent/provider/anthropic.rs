//! Anthropic-backed [`AgentProvider`].

use futures::future::BoxFuture;
use rig::agent::AgentBuilder;
use rig::providers::anthropic;

use super::AgentProvider;
use crate::agent::AgentError;
use crate::config::AnthropicConfig;

fn max_tokens_for_model(model: &str) -> anthropic::MaxTokens {
    if anthropic::ANTHROPIC.default_max_tokens(model).is_some() {
        anthropic::MaxTokens::ByModel
    } else {
        anthropic::MaxTokens::Fixed(128_000)
    }
}

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
        let model = config.model.clone().unwrap();
        let mut provider_config = anthropic::AnthropicConfig::new(api_key)
            .with_base_url(config.base_url.clone().unwrap());
        provider_config.dialect.quirks.max_tokens = max_tokens_for_model(&model);
        let client = provider_config.client();
        Ok(Self {
            client,
            model,
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

#[cfg(test)]
mod tests {
    use super::max_tokens_for_model;
    use rig::providers::anthropic::MaxTokens;

    #[test]
    fn uses_rig_model_limit_when_available() {
        assert_eq!(
            max_tokens_for_model("claude-haiku-4-5"),
            MaxTokens::ByModel
        );
    }

    #[test]
    fn falls_back_to_128k_for_unknown_model_limit() {
        assert_eq!(
            max_tokens_for_model("claude-haiku-5-5"),
            MaxTokens::Fixed(128_000)
        );
    }
}
