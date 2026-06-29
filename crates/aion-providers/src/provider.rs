use std::env;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use aion_config::config::{Config, ProviderType};
use aion_types::llm::{LlmEvent, LlmRequest};

use crate::anthropic;
use crate::bedrock;
use crate::error::ProviderError;
use crate::openai;
use crate::vertex;

/// Unified interface for LLM API providers
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn stream(&self, request: &LlmRequest)
    -> Result<mpsc::Receiver<LlmEvent>, ProviderError>;
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
                .or_else(|| env::var("AWS_REGION").ok())
                .or_else(|| env::var("AWS_DEFAULT_REGION").ok())
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aion_config::compact::CompactConfig;
    use aion_config::compat::ProviderCompat;
    use aion_config::config::{
        BedrockConfig, Config, McpConfig, ProviderType, SessionConfig, ToolsConfig, VertexConfig,
    };
    use aion_config::file_cache::FileCacheConfig;
    use aion_config::hooks::HooksConfig;
    use aion_config::logging::LoggingConfig;
    use aion_config::plan::PlanConfig;
    use aion_config::shell::ShellConfig;

    use super::create_provider;

    fn config_for(provider: ProviderType) -> Config {
        let (provider_label, api_key, base_url, model, prompt_caching, compat, bedrock, vertex) =
            match provider {
                ProviderType::Anthropic => (
                    "anthropic",
                    "test-anthropic-key",
                    "https://api.anthropic.com",
                    "claude-sonnet-4-20250514",
                    true,
                    ProviderCompat::anthropic_defaults(),
                    None,
                    None,
                ),
                ProviderType::OpenAI => (
                    "openai",
                    "test-openai-key",
                    "https://api.openai.com",
                    "gpt-4o",
                    false,
                    ProviderCompat::openai_defaults(),
                    None,
                    None,
                ),
                ProviderType::Bedrock => (
                    "bedrock",
                    "",
                    "",
                    "anthropic.claude-sonnet-4-20250514-v1:0",
                    true,
                    ProviderCompat::bedrock_defaults(),
                    Some(BedrockConfig {
                        region: Some("us-east-1".to_string()),
                        ..Default::default()
                    }),
                    None,
                ),
                ProviderType::Vertex => (
                    "vertex",
                    "",
                    "",
                    "claude-sonnet-4@20250514",
                    true,
                    ProviderCompat::anthropic_defaults(),
                    None,
                    Some(VertexConfig {
                        project_id: Some("test-project".to_string()),
                        region: Some("us-central1".to_string()),
                        ..Default::default()
                    }),
                ),
            };

        Config {
            provider_label: provider_label.to_string(),
            provider,
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            model: model.to_string(),
            max_tokens: 1024,
            max_turns: Some(20),
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            thinking: None,
            prompt_caching,
            compat,
            tools: ToolsConfig::default(),
            session: SessionConfig::default(),
            compact: CompactConfig::default(),
            plan: PlanConfig::default(),
            shell: ShellConfig::default(),
            file_cache: FileCacheConfig::default(),
            hooks: HooksConfig::default(),
            bedrock,
            vertex,
            mcp: McpConfig::default(),
            logging: LoggingConfig::default(),
        }
    }

    #[test]
    fn create_provider_constructs_all_builtin_provider_variants() {
        for provider_type in [
            ProviderType::Anthropic,
            ProviderType::OpenAI,
            ProviderType::Bedrock,
            ProviderType::Vertex,
        ] {
            let config = config_for(provider_type);
            let provider = create_provider(&config);

            assert_eq!(Arc::strong_count(&provider), 1);
        }
    }
}
