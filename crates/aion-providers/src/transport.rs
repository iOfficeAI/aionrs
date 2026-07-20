use aion_config::compat::{OpenAiApiMode, ProviderCompat};
use aion_types::llm::LlmRequest;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::Value;

use crate::bedrock::BedrockTransportState;
use crate::error::ProviderError;
use crate::openai_responses_projector::OpenAiResponsesProjector;
use crate::projector::{
    AnthropicWireProjector, OpenAiProjector, ResolvedToolWireShape, WireParams, WireProvider,
    classify_tools_wire_shape_mismatch, projection_to_provider_error,
};
use crate::retry::MAX_STREAM_RETRIES;
use crate::stream_process::StreamDecoder;
use crate::stream_runner::RetryPolicy;
use crate::vertex::VertexTransportState;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WireProtocol {
    OpenAiChat,
    OpenAiResponses,
    AnthropicMessages,
}

#[derive(Clone)]
pub(crate) enum ProviderTransport {
    OpenAi(OpenAiTransport),
    Anthropic(AnthropicTransport),
    Vertex(VertexTransport),
    Bedrock(BedrockTransport),
}

#[derive(Clone)]
pub(crate) struct OpenAiTransport {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

#[derive(Clone)]
pub(crate) struct AnthropicTransport {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    cache_enabled: bool,
}

#[derive(Clone)]
pub(crate) struct VertexTransport {
    pub(crate) inner: VertexTransportState,
}

#[derive(Clone)]
pub(crate) struct BedrockTransport {
    pub(crate) inner: BedrockTransportState,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectedHttpRequest {
    pub url: String,
    pub headers: HeaderMap,
    pub body: Value,
    pub body_bytes: Option<Vec<u8>>,
    pub tool_wire_shape: ResolvedToolWireShape,
}

impl OpenAiTransport {
    pub(crate) fn new(api_key: &str, base_url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
            base_url: normalize_openai_base_url(base_url),
        }
    }

    pub(crate) fn build_projected_request(
        &self,
        body: Value,
        compat: &ProviderCompat,
        tool_wire_shape: ResolvedToolWireShape,
    ) -> Result<ProjectedHttpRequest, ProviderError> {
        let mut headers = HeaderMap::new();
        let bearer = format!("Bearer {}", self.api_key);
        let auth = HeaderValue::from_str(&bearer)
            .map_err(|error| ProviderError::Connection(format!("Invalid authorization header: {error}")))?;
        headers.insert(AUTHORIZATION, auth);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        Ok(ProjectedHttpRequest {
            url: join_base_url_and_api_path(&self.base_url, compat.openai_api_path()),
            headers,
            body,
            body_bytes: None,
            tool_wire_shape,
        })
    }

    pub(crate) async fn send(&self, request: ProjectedHttpRequest) -> Result<reqwest::Response, ProviderError> {
        send_projected_json_request(&self.client, request).await
    }
}

impl AnthropicTransport {
    pub(crate) fn new(api_key: &str, base_url: &str, cache_enabled: bool) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            cache_enabled,
        }
    }

    pub(crate) fn build_projected_request(
        &self,
        body: Value,
        tool_wire_shape: ResolvedToolWireShape,
    ) -> Result<ProjectedHttpRequest, ProviderError> {
        let mut headers = HeaderMap::new();
        let api_key = HeaderValue::from_str(&self.api_key)
            .map_err(|error| ProviderError::Connection(format!("Invalid x-api-key header: {error}")))?;
        headers.insert("x-api-key", api_key);
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if self.cache_enabled {
            headers.insert("anthropic-beta", HeaderValue::from_static("prompt-caching-2024-07-31"));
        }

        Ok(ProjectedHttpRequest {
            url: format!("{}/v1/messages", self.base_url),
            headers,
            body,
            body_bytes: None,
            tool_wire_shape,
        })
    }

    pub(crate) async fn send(&self, request: ProjectedHttpRequest) -> Result<reqwest::Response, ProviderError> {
        send_projected_json_request(&self.client, request).await
    }
}

impl ProviderTransport {
    #[cfg(test)]
    pub(crate) fn wire_protocol(&self, compat: &ProviderCompat) -> WireProtocol {
        match self {
            Self::OpenAi(_) => match compat.openai_api_mode() {
                OpenAiApiMode::ChatCompletions => WireProtocol::OpenAiChat,
                OpenAiApiMode::Responses => WireProtocol::OpenAiResponses,
            },
            Self::Anthropic(_) | Self::Vertex(_) | Self::Bedrock(_) => WireProtocol::AnthropicMessages,
        }
    }

    pub(crate) fn retry_policy(&self) -> RetryPolicy {
        match self {
            Self::OpenAi(_) => RetryPolicy::new(MAX_STREAM_RETRIES, true, true, true),
            Self::Anthropic(_) | Self::Vertex(_) => RetryPolicy::new(MAX_STREAM_RETRIES, false, true, true),
            Self::Bedrock(_) => RetryPolicy::new(0, false, false, true),
        }
    }

    pub(crate) fn decoder(&self, compat: &ProviderCompat) -> StreamDecoder {
        match self {
            Self::OpenAi(_) => match compat.openai_api_mode() {
                OpenAiApiMode::ChatCompletions => StreamDecoder::OpenAiSseLine {
                    auto_tool_id: compat.auto_tool_id(),
                },
                OpenAiApiMode::Responses => StreamDecoder::OpenAiResponsesSse,
            },
            Self::Anthropic(_) | Self::Vertex(_) => StreamDecoder::AnthropicSseBlock,
            Self::Bedrock(_) => StreamDecoder::BedrockAwsEventStream,
        }
    }

    pub(crate) fn project_body(
        &self,
        request: &LlmRequest,
        compat: &ProviderCompat,
    ) -> Result<(Value, ResolvedToolWireShape), ProviderError> {
        match self {
            Self::OpenAi(_) => match compat.openai_api_mode() {
                OpenAiApiMode::ChatCompletions => {
                    let body = OpenAiProjector::project(request, compat).map_err(projection_to_provider_error)?;
                    Ok((body, OpenAiProjector::resolved_tool_wire_shape(compat)))
                }
                OpenAiApiMode::Responses => {
                    let body =
                        OpenAiResponsesProjector::project(request, compat).map_err(projection_to_provider_error)?;
                    Ok((body, ResolvedToolWireShape::OpenAiFunction))
                }
            },

            Self::Anthropic(transport) => {
                let params = WireParams {
                    provider: WireProvider::Anthropic,
                    anthropic_version: None,
                    include_model_in_body: true,
                    include_stream: true,
                    cache_enabled: transport.cache_enabled,
                    sanitize_schema: false,
                };
                let body =
                    AnthropicWireProjector::project(request, compat, params).map_err(projection_to_provider_error)?;
                Ok((body, AnthropicWireProjector::resolved_tool_wire_shape(compat)))
            }

            Self::Vertex(transport) => {
                let body = AnthropicWireProjector::project(request, compat, transport.inner.wire_params())
                    .map_err(projection_to_provider_error)?;
                Ok((body, AnthropicWireProjector::resolved_tool_wire_shape(compat)))
            }

            Self::Bedrock(transport) => {
                let body = AnthropicWireProjector::project(request, compat, transport.inner.wire_params(compat))
                    .map_err(projection_to_provider_error)?;
                Ok((body, AnthropicWireProjector::resolved_tool_wire_shape(compat)))
            }
        }
    }

    pub(crate) fn build_projected_request(
        &self,
        model: &str,
        body: Value,
        compat: &ProviderCompat,
        tool_wire_shape: ResolvedToolWireShape,
    ) -> Result<ProjectedHttpRequest, ProviderError> {
        match self {
            Self::OpenAi(transport) => transport.build_projected_request(body, compat, tool_wire_shape),
            Self::Anthropic(transport) => transport.build_projected_request(body, tool_wire_shape),
            Self::Vertex(transport) => transport
                .inner
                .build_projected_request(model, body, compat, tool_wire_shape),
            Self::Bedrock(transport) => transport
                .inner
                .build_projected_request(model, body, compat, tool_wire_shape),
        }
    }

    pub(crate) async fn send(&self, request: ProjectedHttpRequest) -> Result<reqwest::Response, ProviderError> {
        match self {
            Self::OpenAi(transport) => transport.send(request).await,
            Self::Anthropic(transport) => transport.send(request).await,
            Self::Vertex(transport) => transport.inner.send(request).await,
            Self::Bedrock(transport) => transport.inner.send(request).await,
        }
    }
}

fn join_base_url_and_api_path(base_url: &str, api_path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let path = api_path.trim_start_matches('/');
    if path.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{path}")
    }
}

fn normalize_openai_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.eq_ignore_ascii_case("https://api.openai.com") || trimmed.eq_ignore_ascii_case("http://api.openai.com") {
        return format!("{trimmed}/v1");
    }

    base_url.to_string()
}

async fn send_projected_json_request(
    client: &reqwest::Client,
    request: ProjectedHttpRequest,
) -> Result<reqwest::Response, ProviderError> {
    let ProjectedHttpRequest {
        url,
        headers,
        body,
        body_bytes,
        tool_wire_shape,
    } = request;

    let builder = client.post(&url).headers(headers);
    let response = match body_bytes {
        Some(bytes) => builder.body(bytes).send().await?,
        None => builder.json(&body).send().await?,
    };

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        return Err(map_common_status(status.as_u16(), body_text, tool_wire_shape));
    }

    Ok(response)
}

fn map_common_status(status: u16, body_text: String, tool_wire_shape: ResolvedToolWireShape) -> ProviderError {
    if status == 429 {
        return ProviderError::RateLimited {
            retry_after_ms: 5000,
            body: (!body_text.is_empty()).then_some(body_text),
        };
    }

    if let Some(message) = classify_tools_wire_shape_mismatch(status, &body_text, tool_wire_shape) {
        return ProviderError::Api { status, message };
    }

    ProviderError::Api {
        status,
        message: body_text,
    }
}

#[cfg(test)]
#[path = "transport_test.rs"]
mod transport_test;
