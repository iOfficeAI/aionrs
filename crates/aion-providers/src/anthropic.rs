use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::Value;
use tokio::sync::mpsc;

use aion_types::llm::{LlmEvent, LlmRequest};

use super::anthropic_shared;
use crate::projector::{
    AnthropicWireProjector, WireParams, WireProvider, projection_to_provider_error,
};
use crate::stream_runner::{RetryPolicy, run_stream};
use crate::{LlmProvider, ProviderError};
use aion_config::compat::ProviderCompat;

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    cache_enabled: bool,
    compat: ProviderCompat,
}

impl AnthropicProvider {
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            cache_enabled: true,
            compat,
        }
    }

    pub fn with_cache(mut self, enabled: bool) -> Self {
        self.cache_enabled = enabled;
        self
    }

    fn build_headers(&self) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        let api_key = HeaderValue::from_str(&self.api_key)
            .map_err(|e| ProviderError::Connection(format!("Invalid x-api-key header: {}", e)))?;
        headers.insert("x-api-key", api_key);
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if self.cache_enabled {
            headers.insert(
                "anthropic-beta",
                HeaderValue::from_static("prompt-caching-2024-07-31"),
            );
        }
        Ok(headers)
    }

    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, ProviderError> {
        AnthropicWireProjector::project(
            request,
            &self.compat,
            WireParams {
                provider: WireProvider::Anthropic,
                anthropic_version: None,
                include_model_in_body: true,
                include_stream: true,
                cache_enabled: self.cache_enabled,
                sanitize_schema: false,
            },
        )
        .map_err(projection_to_provider_error)
    }
}

async fn send_anthropic_stream_request(
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    body: Value,
) -> Result<reqwest::Response, ProviderError> {
    let response = client
        .post(&url)
        .headers(headers)
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        if status.as_u16() == 429 {
            return Err(ProviderError::RateLimited {
                retry_after_ms: 5000,
            });
        }
        return Err(ProviderError::Api {
            status: status.as_u16(),
            message: body_text,
        });
    }

    Ok(response)
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = format!("{}/v1/messages", self.base_url);
        let body = self.build_request_body(request)?;

        tracing::debug!(target: "aion_providers", body = %serde_json::to_string_pretty(&body).unwrap_or_default(), "outgoing request");

        let client = self.client.clone();
        let headers = self.build_headers()?;
        let send = {
            let url = url.clone();
            let body = body.clone();
            move || {
                let client = client.clone();
                let url = url.clone();
                let headers = headers.clone();
                let body = body.clone();
                async move { send_anthropic_stream_request(client, url, headers, body).await }
            }
        };
        let process = move |response, tx| async move {
            anthropic_shared::process_sse_stream(response, &tx).await
        };

        run_stream(
            send,
            process,
            RetryPolicy::new(crate::retry::MAX_STREAM_RETRIES, false, true),
        )
        .await
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
        AnthropicProvider::new(
            "test-key",
            "https://example.test",
            ProviderCompat::anthropic_defaults(),
        )
        .with_cache(cache)
    }

    fn areq(
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
        thinking: Option<ThinkingConfig>,
    ) -> LlmRequest {
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
                vec![ContentBlock::Text {
                    text: "go".to_string(),
                }],
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
                vec![ContentBlock::Text {
                    text: "go".to_string(),
                }],
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
                vec![ContentBlock::Text {
                    text: "q".to_string(),
                }],
            )],
            vec![],
            Some(ThinkingConfig::Enabled {
                budget_tokens: 4096,
            }),
        );
        insta::assert_json_snapshot!(
            "anthropic_with_thinking",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }
}
