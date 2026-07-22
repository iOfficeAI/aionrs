use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    use aion_config::compat::{OpenAiApiMode, ProviderCompat};
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
            max_tokens: Some(8192),
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
        let transport = ProviderTransport::OpenAi(OpenAiTransport::new("test-key", "https://example.test"));
        let compat = ProviderCompat::openai_defaults();

        assert_eq!(transport.wire_protocol(&compat), WireProtocol::OpenAiChat);
        assert_eq!(
            transport.decoder(&compat),
            StreamDecoder::OpenAiSseLine { auto_tool_id: true }
        );
    }

    #[test]
    fn openai_transport_selects_responses_wire_and_decoder_when_configured() {
        let transport = ProviderTransport::OpenAi(OpenAiTransport::new("test-key", "https://example.test"));
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.openai_api_mode = Some(OpenAiApiMode::Responses);

        assert_eq!(transport.wire_protocol(&compat), WireProtocol::OpenAiResponses);
        assert_eq!(transport.decoder(&compat), StreamDecoder::OpenAiResponsesSse);
    }

    #[test]
    fn openai_transport_appends_chat_completions_to_configured_base_url() {
        let transport = OpenAiTransport::new("test-key", "https://open.bigmodel.cn/api/paas/v4/");
        let compat = ProviderCompat::openai_defaults();

        let request = transport
            .build_projected_request(
                json!({ "model": "glm-test" }),
                &compat,
                ResolvedToolWireShape::OpenAiFunction,
            )
            .expect("request projection should succeed");

        assert_eq!(request.url, "https://open.bigmodel.cn/api/paas/v4/chat/completions");
    }

    #[test]
    fn openai_transport_normalizes_official_openai_root_base_url() {
        let transport = OpenAiTransport::new("test-key", "https://api.openai.com");
        let compat = ProviderCompat::openai_defaults();

        let request = transport
            .build_projected_request(
                json!({ "model": "gpt-test" }),
                &compat,
                ResolvedToolWireShape::OpenAiFunction,
            )
            .expect("request projection should succeed");

        assert_eq!(request.url, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn openai_transport_uses_responses_path_when_configured() {
        let transport = OpenAiTransport::new("test-key", "https://api.openai.com");
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.openai_api_mode = Some(OpenAiApiMode::Responses);

        let request = transport
            .build_projected_request(
                json!({ "model": "gpt-5.6-sol" }),
                &compat,
                ResolvedToolWireShape::OpenAiFunction,
            )
            .expect("request projection should succeed");

        assert_eq!(request.url, "https://api.openai.com/v1/responses");
    }

    #[test]
    fn openai_transport_custom_api_path_overrides_default_chat_path() {
        let transport = OpenAiTransport::new("test-key", "https://open.bigmodel.cn/api/paas/v4/");
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.api_path = Some("/custom/chat".to_string());

        let request = transport
            .build_projected_request(
                json!({ "model": "glm-test" }),
                &compat,
                ResolvedToolWireShape::OpenAiFunction,
            )
            .expect("request projection should succeed");

        assert_eq!(request.url, "https://open.bigmodel.cn/api/paas/v4/custom/chat");
    }

    #[test]
    fn anthropic_transport_selects_messages_wire_and_anthropic_decoder() {
        let transport = ProviderTransport::Anthropic(AnthropicTransport::new("test-key", "https://example.test", true));
        let compat = ProviderCompat::anthropic_defaults();

        assert_eq!(transport.wire_protocol(&compat), WireProtocol::AnthropicMessages);
        assert_eq!(transport.decoder(&compat), StreamDecoder::AnthropicSseBlock);
    }

    #[test]
    fn vertex_transport_projects_vertex_anthropic_body_shape() {
        let transport = ProviderTransport::Vertex(VertexTransport {
            inner: VertexTransportState::new("test-project", "us-central1", GcpAuth::ApplicationDefault, false),
        });
        let request = test_request(vec![test_tool()]);
        let compat = ProviderCompat::anthropic_defaults();

        let (body, tool_wire_shape) = transport
            .project_body(&request, &compat)
            .expect("request body projection should succeed");

        assert_eq!(transport.wire_protocol(&compat), WireProtocol::AnthropicMessages);
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

        assert_eq!(transport.wire_protocol(&compat), WireProtocol::AnthropicMessages);
        assert_eq!(transport.retry_policy(), RetryPolicy::new(0, false, false, true));
        assert_eq!(tool_wire_shape, ResolvedToolWireShape::AnthropicInputSchema);
        assert_eq!(body["anthropic_version"], "bedrock-2023-05-31");
        assert!(body.get("model").is_none());
        assert!(body.get("stream").is_none());
        assert!(body["tools"][0]["input_schema"].get("additionalProperties").is_none());
    }

    #[test]
    fn openai_transport_projects_openai_chat_body_shape() {
        let transport = ProviderTransport::OpenAi(OpenAiTransport::new("test-key", "https://example.test"));
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

    #[test]
    fn openai_transport_projects_responses_body_shape_when_configured() {
        let transport = ProviderTransport::OpenAi(OpenAiTransport::new("test-key", "https://example.test"));
        let request = test_request(vec![test_tool()]);
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.openai_api_mode = Some(OpenAiApiMode::Responses);

        let (body, tool_wire_shape) = transport
            .project_body(&request, &compat)
            .expect("request body projection should succeed");

        assert_eq!(tool_wire_shape, ResolvedToolWireShape::OpenAiFunction);
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["stream"], true);
        assert!(body.get("messages").is_none());
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["tools"][0]["type"], "function");
        assert!(body["tools"][0].get("function").is_none());
    }

    #[tokio::test]
    async fn openai_transport_maps_429_to_rate_limited() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
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

        match error {
            ProviderError::RateLimited { retry_after_ms, body } => {
                assert_eq!(retry_after_ms, 5000);
                assert_eq!(body.as_deref(), Some("Too Many Requests"));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn openai_transport_maps_successful_json_502_to_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "error": {
                    "message": "Upstream error from Nvidia: ResourceExhausted: Worker local total request limit reached (33/32)",
                    "code": 502
                }
            })))
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
            .expect_err("embedded 502 should map to an API error");

        assert!(matches!(
            error,
            ProviderError::Api { status: 502, message }
                if message == "Upstream error from Nvidia: ResourceExhausted: Worker local total request limit reached (33/32)"
        ));
    }

    #[tokio::test]
    async fn openai_transport_maps_successful_json_429_to_rate_limited() {
        let server = MockServer::start().await;
        let response_body = json!({
            "error": {
                "message": "Too many requests",
                "code": "429"
            }
        });
        let response_body_text = response_body.to_string();
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
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
            .expect_err("embedded 429 should map to rate limited");

        match error {
            ProviderError::RateLimited { retry_after_ms, body } => {
                assert_eq!(retry_after_ms, 5000);
                assert_eq!(body.as_deref(), Some(response_body_text.as_str()));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn successful_json_top_level_error_shape_is_normalized() {
        let error = map_success_json_response(r#"{"code":"503","message":"upstream busy"}"#);

        assert!(matches!(
            error,
            ProviderError::Api { status: 503, message } if message == "upstream busy"
        ));
    }

    #[test]
    fn successful_json_uses_http_status_when_provider_code_is_not_http() {
        let error = map_success_json_response(r#"{"error":{"code":1001,"message":"upstream busy"},"status":503}"#);

        assert!(matches!(
            error,
            ProviderError::Api { status: 503, message } if message == "upstream busy"
        ));
    }

    #[test]
    fn successful_json_without_error_status_is_a_parse_error() {
        let error = map_success_json_response(r#"{"choices":[]}"#);

        assert!(matches!(
            error,
            ProviderError::Parse(message) if message.contains("unexpected JSON response")
        ));
    }

    #[tokio::test]
    async fn openai_transport_preserves_429_body_as_none_when_empty() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_string(""))
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
            .expect_err("empty-body 429 should still map to rate limited");

        assert!(matches!(
            error,
            ProviderError::RateLimited {
                retry_after_ms: 5000,
                body: None,
            }
        ));
    }

    #[tokio::test]
    async fn openai_transport_preserves_generic_api_error_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
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

        let error = transport.send(request).await.expect_err("500 should map to api error");

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
                ResponseTemplate::new(400).set_body_string("invalid_request_error: body.tools[0].function is missing"),
            )
            .mount(&server)
            .await;
        let transport = ProviderTransport::Anthropic(AnthropicTransport::new("test-key", &server.uri(), false));
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
        let enabled = ProviderTransport::Anthropic(AnthropicTransport::new("test-key", "https://example.test", true))
            .build_projected_request("test-model", body.clone(), &compat, tool_wire_shape)
            .expect("projected request should build");
        let disabled = ProviderTransport::Anthropic(AnthropicTransport::new("test-key", "https://example.test", false))
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
            .build_projected_request("test-model", body, &compat, ResolvedToolWireShape::AnthropicInputSchema)
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
            .respond_with(ResponseTemplate::new(200).set_body_raw("data: [DONE]\n\n", "application/json"))
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
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer test-key"))
            .and(header("content-type", "application/json"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("data: [DONE]\n\n", "text/event-stream"))
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
