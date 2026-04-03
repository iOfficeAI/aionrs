pub mod anthropic;
pub mod anthropic_shared;
pub mod bedrock;
pub mod compat;
pub mod openai;
pub mod retry;
pub mod vertex;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::config::{Config, ProviderType};
use crate::types::llm::{LlmEvent, LlmRequest};

/// Unified interface for LLM API providers
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a streaming request, return a channel of events
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },

    #[error("SSE parse error: {0}")]
    Parse(String),

    #[error("Rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },

    #[error("Prompt too long: {0}")]
    PromptTooLong(String),

    #[error("Connection error: {0}")]
    Connection(String),
}

impl ProviderError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ProviderError::RateLimited { .. } | ProviderError::Connection(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderError;

    #[test]
    fn test_provider_error_is_retryable_rate_limit() {
        let err = ProviderError::RateLimited { retry_after_ms: 1000 };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_provider_error_is_retryable_connection() {
        let err = ProviderError::Connection("timeout".into());
        assert!(err.is_retryable());
    }

    #[test]
    fn test_provider_error_not_retryable_api() {
        let err = ProviderError::Api { status: 401, message: "auth".into() };
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_provider_error_not_retryable_parse() {
        let err = ProviderError::Parse("bad json".into());
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_provider_error_not_retryable_prompt_too_long() {
        let err = ProviderError::PromptTooLong("too long".into());
        assert!(!err.is_retryable());
    }
}

/// Create a provider based on config
pub fn create_provider(config: &Config) -> Arc<dyn LlmProvider> {
    let compat = config.compat.clone();

    match config.provider {
        ProviderType::Anthropic => Arc::new(
            anthropic::AnthropicProvider::new(&config.api_key, &config.base_url, compat)
                .with_cache(config.prompt_caching),
        ),
        ProviderType::OpenAI => Arc::new(openai::OpenAIProvider::new(
            &config.api_key,
            &config.base_url,
            compat,
        )),
        ProviderType::Bedrock => {
            let bedrock_config = config.bedrock.clone().unwrap_or_default();
            let region = bedrock_config
                .region
                .clone()
                .or_else(|| std::env::var("AWS_REGION").ok())
                .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
                .unwrap_or_else(|| "us-east-1".to_string());
            let credentials = bedrock_config.to_credentials();
            Arc::new(bedrock::BedrockProvider::new(
                &region,
                credentials,
                config.prompt_caching,
                compat,
            ))
        }
        ProviderType::Vertex => {
            let vertex_config = config.vertex.clone().unwrap_or_default();
            let project_id = vertex_config
                .project_id
                .clone()
                .unwrap_or_default();
            let region = vertex_config
                .region
                .clone()
                .unwrap_or_else(|| "us-central1".to_string());
            let auth = vertex_config.to_auth();
            Arc::new(vertex::VertexProvider::new(
                &project_id,
                &region,
                auth,
                config.prompt_caching,
                compat,
            ))
        }
    }
}
