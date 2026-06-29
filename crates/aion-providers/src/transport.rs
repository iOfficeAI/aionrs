use aion_config::compat::ProviderCompat;
use aion_types::llm::LlmRequest;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::Value;

use crate::bedrock::BedrockTransportState;
use crate::error::ProviderError;
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
            base_url: base_url.to_string(),
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
        let auth = HeaderValue::from_str(&bearer).map_err(|error| {
            ProviderError::Connection(format!("Invalid authorization header: {error}"))
        })?;
        headers.insert(AUTHORIZATION, auth);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        Ok(ProjectedHttpRequest {
            url: format!("{}{}", self.base_url, compat.api_path()),
            headers,
            body,
            body_bytes: None,
            tool_wire_shape,
        })
    }

    pub(crate) async fn send(
        &self,
        request: ProjectedHttpRequest,
    ) -> Result<reqwest::Response, ProviderError> {
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
        let api_key = HeaderValue::from_str(&self.api_key).map_err(|error| {
            ProviderError::Connection(format!("Invalid x-api-key header: {error}"))
        })?;
        headers.insert("x-api-key", api_key);
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if self.cache_enabled {
            headers.insert(
                "anthropic-beta",
                HeaderValue::from_static("prompt-caching-2024-07-31"),
            );
        }

        Ok(ProjectedHttpRequest {
            url: format!("{}/v1/messages", self.base_url),
            headers,
            body,
            body_bytes: None,
            tool_wire_shape,
        })
    }

    pub(crate) async fn send(
        &self,
        request: ProjectedHttpRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        send_projected_json_request(&self.client, request).await
    }
}

impl ProviderTransport {
    #[cfg(test)]
    pub(crate) fn wire_protocol(&self) -> WireProtocol {
        match self {
            Self::OpenAi(_) => WireProtocol::OpenAiChat,
            Self::Anthropic(_) | Self::Vertex(_) | Self::Bedrock(_) => {
                WireProtocol::AnthropicMessages
            }
        }
    }

    pub(crate) fn retry_policy(&self) -> RetryPolicy {
        match self {
            Self::OpenAi(_) => RetryPolicy::new(MAX_STREAM_RETRIES, true, true),
            Self::Anthropic(_) | Self::Vertex(_) => {
                RetryPolicy::new(MAX_STREAM_RETRIES, false, true)
            }
            Self::Bedrock(_) => RetryPolicy::new(0, false, false),
        }
    }

    pub(crate) fn decoder(&self, compat: &ProviderCompat) -> StreamDecoder {
        match self {
            Self::OpenAi(_) => StreamDecoder::OpenAiSseLine {
                auto_tool_id: compat.auto_tool_id(),
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
            Self::OpenAi(_) => {
                let body = OpenAiProjector::project(request, compat)
                    .map_err(projection_to_provider_error)?;
                Ok((body, OpenAiProjector::resolved_tool_wire_shape(compat)))
            }

            Self::Anthropic(transport) => {
                let params = WireParams {
                    provider: WireProvider::Anthropic,
                    anthropic_version: None,
                    include_model_in_body: true,
                    include_stream: true,
                    cache_enabled: transport.cache_enabled,
                    sanitize_schema: false,
                };
                let body = AnthropicWireProjector::project(request, compat, params)
                    .map_err(projection_to_provider_error)?;
                Ok((
                    body,
                    AnthropicWireProjector::resolved_tool_wire_shape(compat),
                ))
            }

            Self::Vertex(transport) => {
                let body =
                    AnthropicWireProjector::project(request, compat, transport.inner.wire_params())
                        .map_err(projection_to_provider_error)?;
                Ok((
                    body,
                    AnthropicWireProjector::resolved_tool_wire_shape(compat),
                ))
            }

            Self::Bedrock(transport) => {
                let body = AnthropicWireProjector::project(
                    request,
                    compat,
                    transport.inner.wire_params(compat),
                )
                .map_err(projection_to_provider_error)?;
                Ok((
                    body,
                    AnthropicWireProjector::resolved_tool_wire_shape(compat),
                ))
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
            Self::OpenAi(transport) => {
                transport.build_projected_request(body, compat, tool_wire_shape)
            }
            Self::Anthropic(transport) => transport.build_projected_request(body, tool_wire_shape),
            Self::Vertex(transport) => {
                transport
                    .inner
                    .build_projected_request(model, body, compat, tool_wire_shape)
            }
            Self::Bedrock(transport) => {
                transport
                    .inner
                    .build_projected_request(model, body, compat, tool_wire_shape)
            }
        }
    }

    pub(crate) async fn send(
        &self,
        request: ProjectedHttpRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        match self {
            Self::OpenAi(transport) => transport.send(request).await,
            Self::Anthropic(transport) => transport.send(request).await,
            Self::Vertex(transport) => transport.inner.send(request).await,
            Self::Bedrock(transport) => transport.inner.send(request).await,
        }
    }
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
        return Err(map_common_status(
            status.as_u16(),
            body_text,
            tool_wire_shape,
        ));
    }

    Ok(response)
}

fn map_common_status(
    status: u16,
    body_text: String,
    tool_wire_shape: ResolvedToolWireShape,
) -> ProviderError {
    if status == 429 {
        return ProviderError::RateLimited {
            retry_after_ms: 5000,
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
mod tests {
    use super::*;

    use aion_config::compat::ProviderCompat;
    use aion_types::llm::LlmRequest;
    use aion_types::message::{ContentBlock, Message, Role};
    use aion_types::tool::ToolDef;
    use serde_json::json;
    use wiremock::matchers::{body_bytes, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::bedrock::{AwsCredentials, BedrockTransportState};
    use crate::error::ProviderError;
    use crate::projector::ResolvedToolWireShape;
    use crate::stream_process::StreamDecoder;
    use crate::stream_runner::RetryPolicy;
    use crate::vertex::{GcpAuth, VertexTransportState};

    fn test_request(tools: Vec<ToolDef>) -> LlmRequest {
        LlmRequest {
            model: "test-model".to_string(),
            system: "You are a test assistant.".to_string(),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            tools,
            max_tokens: 8192,
            thinking: None,
            reasoning_effort: None,
        }
    }

    fn test_tool() -> ToolDef {
        ToolDef {
            name: "read".to_string(),
            description: "Read".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": ["string", "null"]}},
                "additionalProperties": false
            }),
            deferred: false,
        }
    }

    #[test]
    fn openai_transport_selects_chat_wire_and_openai_decoder() {
        let transport =
            ProviderTransport::OpenAi(OpenAiTransport::new("test-key", "https://example.test"));
        let compat = ProviderCompat::openai_defaults();

        assert_eq!(transport.wire_protocol(), WireProtocol::OpenAiChat);
        assert_eq!(
            transport.decoder(&compat),
            StreamDecoder::OpenAiSseLine { auto_tool_id: true }
        );
    }

    #[test]
    fn anthropic_transport_selects_messages_wire_and_anthropic_decoder() {
        let transport = ProviderTransport::Anthropic(AnthropicTransport::new(
            "test-key",
            "https://example.test",
            true,
        ));
        let compat = ProviderCompat::anthropic_defaults();

        assert_eq!(transport.wire_protocol(), WireProtocol::AnthropicMessages);
        assert_eq!(transport.decoder(&compat), StreamDecoder::AnthropicSseBlock);
    }

    #[test]
    fn vertex_transport_projects_vertex_anthropic_body_shape() {
        let transport = ProviderTransport::Vertex(VertexTransport {
            inner: VertexTransportState::new(
                "test-project",
                "us-central1",
                GcpAuth::ApplicationDefault,
                false,
            ),
        });
        let request = test_request(vec![test_tool()]);
        let compat = ProviderCompat::anthropic_defaults();

        let (body, tool_wire_shape) = transport
            .project_body(&request, &compat)
            .expect("request body projection should succeed");

        assert_eq!(transport.wire_protocol(), WireProtocol::AnthropicMessages);
        assert_eq!(tool_wire_shape, ResolvedToolWireShape::AnthropicInputSchema);
        assert_eq!(body["anthropic_version"], "vertex-2023-10-16");
        assert_eq!(body["stream"], true);
        assert!(body.get("model").is_none());
        assert!(body["tools"][0].get("input_schema").is_some());
        assert!(body["tools"][0].get("function").is_none());
    }

    #[test]
    fn bedrock_transport_uses_no_retry_and_projects_body_without_model_or_stream() {
        let transport = ProviderTransport::Bedrock(BedrockTransport {
            inner: BedrockTransportState::new(
                "us-east-1",
                AwsCredentials::Explicit {
                    access_key_id: "test-key".to_string(),
                    secret_access_key: "test-secret".to_string(),
                    session_token: None,
                },
                false,
            ),
        });
        let request = test_request(vec![test_tool()]);
        let compat = ProviderCompat::bedrock_defaults();

        let (body, tool_wire_shape) = transport
            .project_body(&request, &compat)
            .expect("request body projection should succeed");

        assert_eq!(transport.wire_protocol(), WireProtocol::AnthropicMessages);
        assert_eq!(transport.retry_policy(), RetryPolicy::new(0, false, false));
        assert_eq!(tool_wire_shape, ResolvedToolWireShape::AnthropicInputSchema);
        assert_eq!(body["anthropic_version"], "bedrock-2023-05-31");
        assert!(body.get("model").is_none());
        assert!(body.get("stream").is_none());
        assert!(
            body["tools"][0]["input_schema"]
                .get("additionalProperties")
                .is_none()
        );
    }

    #[test]
    fn openai_transport_projects_openai_chat_body_shape() {
        let transport =
            ProviderTransport::OpenAi(OpenAiTransport::new("test-key", "https://example.test"));
        let request = test_request(vec![test_tool()]);
        let compat = ProviderCompat::openai_defaults();

        let (body, tool_wire_shape) = transport
            .project_body(&request, &compat)
            .expect("request body projection should succeed");

        assert_eq!(tool_wire_shape, ResolvedToolWireShape::OpenAiFunction);
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        assert!(body["tools"][0].get("function").is_some());
        assert!(body["tools"][0].get("input_schema").is_none());
    }

    #[tokio::test]
    async fn openai_transport_maps_429_to_rate_limited() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_string("Too Many Requests"))
            .mount(&server)
            .await;
        let transport = ProviderTransport::OpenAi(OpenAiTransport::new("test-key", &server.uri()));
        let compat = ProviderCompat::openai_defaults();
        let (body, tool_wire_shape) = transport
            .project_body(&test_request(vec![]), &compat)
            .expect("request body projection should succeed");
        let request = transport
            .build_projected_request("test-model", body, &compat, tool_wire_shape)
            .expect("projected request should build");

        let error = transport
            .send(request)
            .await
            .expect_err("429 should map to rate limited");

        assert!(matches!(
            error,
            ProviderError::RateLimited {
                retry_after_ms: 5000
            }
        ));
    }

    #[tokio::test]
    async fn openai_transport_preserves_generic_api_error_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("upstream exploded"))
            .mount(&server)
            .await;
        let transport = ProviderTransport::OpenAi(OpenAiTransport::new("test-key", &server.uri()));
        let compat = ProviderCompat::openai_defaults();
        let (body, tool_wire_shape) = transport
            .project_body(&test_request(vec![]), &compat)
            .expect("request body projection should succeed");
        let request = transport
            .build_projected_request("test-model", body, &compat, tool_wire_shape)
            .expect("projected request should build");

        let error = transport
            .send(request)
            .await
            .expect_err("500 should map to api error");

        assert!(matches!(
            error,
            ProviderError::Api { status: 500, message } if message == "upstream exploded"
        ));
    }

    #[tokio::test]
    async fn anthropic_transport_maps_tool_shape_mismatch_to_actionable_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_string("invalid_request_error: body.tools[0].function is missing"),
            )
            .mount(&server)
            .await;
        let transport =
            ProviderTransport::Anthropic(AnthropicTransport::new("test-key", &server.uri(), false));
        let compat = ProviderCompat::anthropic_defaults();
        let (body, tool_wire_shape) = transport
            .project_body(&test_request(vec![test_tool()]), &compat)
            .expect("request body projection should succeed");
        let request = transport
            .build_projected_request("test-model", body, &compat, tool_wire_shape)
            .expect("projected request should build");

        let error = transport
            .send(request)
            .await
            .expect_err("tool shape mismatch should map to api error");

        assert!(matches!(
            error,
            ProviderError::Api { status: 400, message }
                if message.contains("tools wire shape mismatch")
                    && message.contains("anthropic_input_schema")
                    && message.contains("openai_function")
        ));
    }

    #[test]
    fn anthropic_projected_request_includes_cache_beta_when_enabled() {
        let compat = ProviderCompat::anthropic_defaults();
        let body = json!({"model": "test-model"});
        let tool_wire_shape = ResolvedToolWireShape::AnthropicInputSchema;
        let enabled = ProviderTransport::Anthropic(AnthropicTransport::new(
            "test-key",
            "https://example.test",
            true,
        ))
        .build_projected_request("test-model", body.clone(), &compat, tool_wire_shape)
        .expect("projected request should build");
        let disabled = ProviderTransport::Anthropic(AnthropicTransport::new(
            "test-key",
            "https://example.test",
            false,
        ))
        .build_projected_request("test-model", body, &compat, tool_wire_shape)
        .expect("projected request should build");

        assert_eq!(
            enabled
                .headers
                .get("anthropic-beta")
                .and_then(|value| value.to_str().ok()),
            Some("prompt-caching-2024-07-31")
        );
        assert!(disabled.headers.get("anthropic-beta").is_none());
    }

    #[test]
    fn bedrock_projected_request_sends_the_signed_body_bytes() {
        let bedrock = ProviderTransport::Bedrock(BedrockTransport {
            inner: BedrockTransportState::new(
                "us-east-1",
                AwsCredentials::Explicit {
                    access_key_id: "test-key".to_string(),
                    secret_access_key: "test-secret".to_string(),
                    session_token: None,
                },
                false,
            ),
        });
        let compat = ProviderCompat::bedrock_defaults();
        let body = json!({"anthropic_version": "bedrock-2023-05-31", "messages": []});
        let expected_body_bytes = serde_json::to_vec(&body).expect("test body should serialize");
        assert_eq!(
            expected_body_bytes.as_slice(),
            br#"{"anthropic_version":"bedrock-2023-05-31","messages":[]}"#
        );

        let request = bedrock
            .build_projected_request(
                "test-model",
                body,
                &compat,
                ResolvedToolWireShape::AnthropicInputSchema,
            )
            .expect("bedrock projected request should build");

        assert_eq!(request.body_bytes, Some(expected_body_bytes));
        assert!(request.headers.get(AUTHORIZATION).is_some());
        assert_eq!(
            request
                .headers
                .get("x-amz-content-sha256")
                .and_then(|value| value.to_str().ok()),
            Some("7d0653676f838fb8c759e4167a61f2c282ac2c36cf1d34d2c3791f1474e97b0e")
        );
    }

    #[tokio::test]
    async fn bedrock_transport_send_requires_signed_body_bytes() {
        let bedrock = ProviderTransport::Bedrock(BedrockTransport {
            inner: BedrockTransportState::new(
                "us-east-1",
                AwsCredentials::Explicit {
                    access_key_id: "test-key".to_string(),
                    secret_access_key: "test-secret".to_string(),
                    session_token: None,
                },
                false,
            ),
        });
        let request = ProjectedHttpRequest {
            url: "https://example.test".to_string(),
            headers: HeaderMap::new(),
            body: json!({}),
            body_bytes: None,
            tool_wire_shape: ResolvedToolWireShape::AnthropicInputSchema,
        };

        let bedrock_error = bedrock
            .send(request)
            .await
            .expect_err("bedrock send should require signed body bytes");

        assert!(matches!(
            bedrock_error,
            ProviderError::Connection(message)
                if message == "Bedrock projected request missing signed request body bytes"
        ));
    }

    #[tokio::test]
    async fn bedrock_transport_send_uses_projected_body_bytes_over_json_body() {
        let server = MockServer::start().await;
        let expected_body_bytes = b"signed-body-bytes".to_vec();
        Mock::given(method("POST"))
            .and(path("/bedrock"))
            .and(body_bytes(expected_body_bytes.clone()))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw("data: [DONE]\n\n", "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let bedrock = ProviderTransport::Bedrock(BedrockTransport {
            inner: BedrockTransportState::new(
                "us-east-1",
                AwsCredentials::Explicit {
                    access_key_id: "test-key".to_string(),
                    secret_access_key: "test-secret".to_string(),
                    session_token: None,
                },
                false,
            ),
        });
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let request = ProjectedHttpRequest {
            url: format!("{}/bedrock", server.uri()),
            headers,
            body: json!({"this": "must not be serialized"}),
            body_bytes: Some(expected_body_bytes),
            tool_wire_shape: ResolvedToolWireShape::AnthropicInputSchema,
        };

        bedrock
            .send(request)
            .await
            .expect("successful response should pass through");
    }

    #[tokio::test]
    async fn openai_projected_request_sends_headers_and_json_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer test-key"))
            .and(header("content-type", "application/json"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw("data: [DONE]\n\n", "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let transport = ProviderTransport::OpenAi(OpenAiTransport::new("test-key", &server.uri()));
        let compat = ProviderCompat::openai_defaults();
        let (body, tool_wire_shape) = transport
            .project_body(&test_request(vec![]), &compat)
            .expect("request body projection should succeed");
        let request = transport
            .build_projected_request("test-model", body, &compat, tool_wire_shape)
            .expect("projected request should build");

        transport
            .send(request)
            .await
            .expect("successful response should pass through");
    }
}
