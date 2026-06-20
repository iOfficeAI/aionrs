use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use aion_types::llm::{LlmRequest, ThinkingConfig};

use super::anthropic_shared;
use crate::{
    LlmProvider, ProviderError, ProviderFailure, ProviderFailurePhase, ProviderStreamReceiver,
    provider_api_error, provider_failure_from_error, retry_after_ms_from_headers,
};
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

    fn build_request_body(&self, request: &LlmRequest) -> Value {
        // Build system prompt with optional cache_control
        let system = if self.cache_enabled {
            json!([{
                "type": "text",
                "text": &request.system,
                "cache_control": { "type": "ephemeral" }
            }])
        } else {
            json!(&request.system)
        };

        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "system": system,
            "messages": anthropic_shared::build_messages(&request.messages, &self.compat),
            "stream": true
        });

        if !request.tools.is_empty() {
            let mut tools = anthropic_shared::build_tools(&request.tools);
            // Mark last tool with cache_control to cache the entire tools block
            if let Some(last) = tools.last_mut().filter(|_| self.cache_enabled) {
                last["cache_control"] = json!({ "type": "ephemeral" });
            }
            body["tools"] = json!(tools);
        }

        if let Some(ThinkingConfig::Enabled { budget_tokens }) = &request.thinking {
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget_tokens
            });
        }

        body
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<ProviderStreamReceiver, ProviderFailure> {
        let url = format!("{}/v1/messages", self.base_url);
        let body = self.build_request_body(request);
        let model = Some(request.model.clone());

        tracing::debug!(target: "aion_providers", body = %serde_json::to_string_pretty(&body).unwrap_or_default(), "outgoing request");

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers().map_err(|error| {
                provider_failure_from_error(
                    "anthropic",
                    model.clone(),
                    error,
                    ProviderFailurePhase::BeforeFirstDelta,
                )
            })?)
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::from)
            .map_err(|error| {
                provider_failure_from_error(
                    "anthropic",
                    model.clone(),
                    error,
                    ProviderFailurePhase::BeforeFirstDelta,
                )
            })?;

        let status = response.status();
        if !status.is_success() {
            let retry_after_ms = retry_after_ms_from_headers(response.headers());
            let body_text = response.text().await.unwrap_or_default();
            return Err(provider_failure_from_error(
                "anthropic",
                model,
                provider_api_error(status.as_u16(), body_text, retry_after_ms),
                ProviderFailurePhase::BeforeFirstDelta,
            ));
        }

        let (tx, rx) = mpsc::channel(64);
        let client = self.client.clone();
        let headers = self.build_headers().map_err(|error| {
            provider_failure_from_error(
                "anthropic",
                model.clone(),
                error,
                ProviderFailurePhase::BeforeFirstDelta,
            )
        })?;
        let url_clone = url.clone();
        let stream_model = model.clone();

        tokio::spawn(async move {
            match anthropic_shared::process_sse_stream(response, &tx).await {
                anthropic_shared::StreamOutcome::Ok => {}
                anthropic_shared::StreamOutcome::FailedPartial(e) => {
                    let failure = provider_failure_from_error(
                        "anthropic",
                        stream_model.clone(),
                        e,
                        ProviderFailurePhase::AfterFirstDelta,
                    );
                    let _ = tx.send(Err(failure)).await;
                }
                anthropic_shared::StreamOutcome::FailedEmpty(e) => {
                    if e.is_retryable() {
                        let mut backoff = std::time::Duration::from_secs(1);
                        let mut final_err = Some((e, ProviderFailurePhase::BeforeFirstDelta));
                        for attempt in 1..=crate::retry::MAX_STREAM_RETRIES {
                            backoff = crate::retry::backoff_sleep(attempt, backoff).await;
                            match crate::retry::send_and_check(&client, &url_clone, &headers, &body)
                                .await
                            {
                                Ok(resp) => {
                                    let outcome =
                                        anthropic_shared::process_sse_stream(resp, &tx).await;
                                    let phase = match &outcome {
                                        anthropic_shared::StreamOutcome::FailedPartial(_) => {
                                            ProviderFailurePhase::AfterFirstDelta
                                        }
                                        anthropic_shared::StreamOutcome::Ok
                                        | anthropic_shared::StreamOutcome::FailedEmpty(_) => {
                                            ProviderFailurePhase::BeforeFirstDelta
                                        }
                                    };
                                    match crate::retry::evaluate_outcome(outcome, attempt) {
                                        Ok(None) => {
                                            final_err = None;
                                            break;
                                        }
                                        Ok(Some(e)) => {
                                            final_err = Some((e, phase));
                                            break;
                                        }
                                        Err(_) => continue,
                                    }
                                }
                                Err(e) if attempt == crate::retry::MAX_STREAM_RETRIES => {
                                    final_err = Some((e, ProviderFailurePhase::BeforeFirstDelta));
                                    break;
                                }
                                Err(_) => continue,
                            }
                        }
                        if let Some((err, phase)) = final_err {
                            let failure = provider_failure_from_error(
                                "anthropic",
                                stream_model.clone(),
                                err,
                                phase,
                            );
                            let _ = tx.send(Err(failure)).await;
                        }
                    } else {
                        let failure = provider_failure_from_error(
                            "anthropic",
                            stream_model.clone(),
                            e,
                            ProviderFailurePhase::BeforeFirstDelta,
                        );
                        let _ = tx.send(Err(failure)).await;
                    }
                }
            }
        });

        Ok(rx)
    }
}
