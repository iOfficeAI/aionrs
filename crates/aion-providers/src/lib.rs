pub mod anthropic;
pub mod anthropic_shared;
pub mod bedrock;
pub mod openai;
pub mod retry;
pub mod vertex;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use aion_config::config::{Config, ProviderType};
use aion_config::debug::DebugConfig;
use aion_types::llm::{LlmEvent, LlmRequest};

/// Unified interface for LLM API providers
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn stream(&self, request: &LlmRequest)
    -> Result<mpsc::Receiver<LlmEvent>, ProviderError>;
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

/// Write the request body to the configured dump path (if set).
///
/// This is a shared helper called by each provider's `stream()` method.
/// Errors are silently ignored — debug output must never break requests.
pub fn dump_request_body(debug: &DebugConfig, body: &serde_json::Value) {
    if let Some(path) = &debug.dump_request_path {
        let pretty = serde_json::to_string_pretty(body).unwrap_or_default();
        let _ = std::fs::write(path, &pretty);
    }
}

/// Truncate the response dump file at the start of a new request.
pub fn reset_response_dump(debug: &DebugConfig) {
    if let Some(path) = &debug.dump_response_path {
        let _ = std::fs::write(path, "");
    }
}

/// Append a raw SSE line to the response dump file.
pub fn dump_response_chunk(debug: &DebugConfig, chunk: &str) {
    if let Some(path) = &debug.dump_response_path {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{chunk}");
        }
    }
}

/// Create a provider from resolved config
pub fn create_provider(config: &Config) -> Arc<dyn LlmProvider> {
    let compat = config.compat.clone();
    let debug = config.debug.clone();

    match config.provider {
        ProviderType::Anthropic => Arc::new(
            anthropic::AnthropicProvider::new(&config.api_key, &config.base_url, compat, debug)
                .with_cache(config.prompt_caching),
        ),
        ProviderType::OpenAI => Arc::new(openai::OpenAIProvider::new(
            &config.api_key,
            &config.base_url,
            compat,
            debug,
        )),
        ProviderType::Bedrock => {
            let bc = config.bedrock.clone().unwrap_or_default();
            let region = bc
                .region
                .clone()
                .or_else(|| std::env::var("AWS_REGION").ok())
                .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
                .unwrap_or_else(|| "us-east-1".to_string());
            let credentials = bedrock::credentials_from_config(&bc);
            Arc::new(bedrock::BedrockProvider::new(
                &region,
                credentials,
                config.prompt_caching,
                compat,
                debug,
            ))
        }
        ProviderType::Vertex => {
            let vc = config.vertex.clone().unwrap_or_default();
            let project_id = vc.project_id.clone().unwrap_or_default();
            let region = vc
                .region
                .clone()
                .unwrap_or_else(|| "us-central1".to_string());
            let auth = vertex::auth_from_config(&vc);
            Arc::new(vertex::VertexProvider::new(
                &project_id,
                &region,
                auth,
                config.prompt_caching,
                compat,
                debug,
            ))
        }
    }
}
