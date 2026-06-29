use async_trait::async_trait;
#[cfg(test)]
use serde_json::Value;
use tokio::sync::mpsc;

use aion_types::llm::{LlmEvent, LlmRequest};

use crate::composed::ComposedProvider;
use crate::transport::{AnthropicTransport, ProviderTransport};
use crate::{LlmProvider, ProviderError};
use aion_config::compat::ProviderCompat;

pub struct AnthropicProvider {
    inner: ComposedProvider,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    cache_enabled: bool,
}

impl AnthropicProvider {
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat) -> Self {
        let cache_enabled = true;
        let inner = Self::build_inner(api_key, base_url, cache_enabled, &compat);

        Self {
            inner,
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            compat,
            cache_enabled,
        }
    }

    pub fn with_cache(mut self, enabled: bool) -> Self {
        self.cache_enabled = enabled;
        self.inner = Self::build_inner(&self.api_key, &self.base_url, self.cache_enabled, &self.compat);
        self
    }

    fn build_inner(api_key: &str, base_url: &str, cache_enabled: bool, compat: &ProviderCompat) -> ComposedProvider {
        let transport = ProviderTransport::Anthropic(AnthropicTransport::new(api_key, base_url, cache_enabled));
        ComposedProvider::new(transport, compat.clone())
    }

    #[cfg(test)]
    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, ProviderError> {
        self.inner.build_request_body(request)
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aion_types::llm::ThinkingConfig;
    use aion_types::message::{ContentBlock, Message, Role};
    use aion_types::tool::ToolDef;
    use serde_json::json;

    fn anthropic_golden(cache: bool) -> AnthropicProvider {
        AnthropicProvider::new("test-key", "https://example.test", ProviderCompat::anthropic_defaults())
            .with_cache(cache)
    }

    fn areq(messages: Vec<Message>, tools: Vec<ToolDef>, thinking: Option<ThinkingConfig>) -> LlmRequest {
        LlmRequest {
            model: "test-model".to_string(),
            system: "You are a test assistant.".to_string(),
            messages,
            tools,
            max_tokens: 8192,
            thinking,
            reasoning_effort: None,
        }
    }

    fn atools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "read".to_string(),
                description: "Read".to_string(),
                input_schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
                deferred: false,
            },
            ToolDef {
                name: "list".to_string(),
                description: "List".to_string(),
                input_schema: json!({"type":"object","properties":{}}),
                deferred: false,
            },
        ]
    }

    #[test]
    fn golden_anthropic_basic() {
        let p = anthropic_golden(false);
        let r = areq(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            vec![],
            None,
        );
        insta::assert_json_snapshot!(
            "anthropic_basic",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_anthropic_with_tools_no_cache() {
        let p = anthropic_golden(false);
        let r = areq(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "go".to_string() }],
            )],
            atools(),
            None,
        );
        insta::assert_json_snapshot!(
            "anthropic_with_tools_no_cache",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_anthropic_with_cache() {
        let p = anthropic_golden(true);
        let r = areq(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "go".to_string() }],
            )],
            atools(),
            None,
        );
        insta::assert_json_snapshot!(
            "anthropic_with_cache",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_anthropic_with_thinking() {
        let p = anthropic_golden(false);
        let r = areq(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "q".to_string() }],
            )],
            vec![],
            Some(ThinkingConfig::Enabled { budget_tokens: 4096 }),
        );
        insta::assert_json_snapshot!(
            "anthropic_with_thinking",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }
}
