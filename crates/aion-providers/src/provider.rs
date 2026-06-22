use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use aion_config::config::{Config, ProviderType};
use aion_types::llm::{LlmEvent, LlmRequest};

use crate::anthropic;
use crate::bedrock;
use crate::error::ProviderFailure;
use crate::openai;
use crate::vertex;

/// Unified interface for LLM API providers
pub type ProviderStreamItem = Result<LlmEvent, ProviderFailure>;
pub type ProviderStreamReceiver = mpsc::Receiver<ProviderStreamItem>;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn stream(&self, request: &LlmRequest)
    -> Result<ProviderStreamReceiver, ProviderFailure>;
}

/// Create a provider from resolved config
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
            ))
        }
    }
}
