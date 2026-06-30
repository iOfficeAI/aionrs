use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use aion_config::compat::ProviderCompat;
    use aion_types::message::{ContentBlock, Message, Role};
    use aion_types::tool::ToolDef;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::projector::ResolvedToolWireShape;
    use crate::transport::{ProjectedHttpRequest, ProviderTransport, VertexTransport};

    // --- Golden body snapshots (baseline for compat-split / seam-extraction refactors) ---

    fn vertex_test_provider() -> VertexProvider {
        VertexProvider::new(
            "test-project",
            "us-central1",
            GcpAuth::ApplicationDefault,
            false,
            ProviderCompat::anthropic_defaults(),
        )
    }

    fn vertex_req(messages: Vec<Message>, tools: Vec<ToolDef>) -> LlmRequest {
        LlmRequest {
            model: "test-model".to_string(),
            system: "You are a test assistant.".to_string(),
            messages,
            tools,
            max_tokens: 8192,
            thinking: None,
            reasoning_effort: None,
        }
    }

    macro_rules! assert_vertex_json_snapshot {
        ($name:literal, $value:expr) => {
            insta::with_settings!({ prepend_module_to_snapshot => false }, {
                insta::assert_json_snapshot!(
                    concat!("aion_providers__vertex__tests__", $name),
                    $value
                );
            });
        };
    }

    #[test]
    fn vertex_provider_preserves_clone_api() {
        fn assert_clone<T: Clone>() {}

        assert_clone::<VertexProvider>();
    }

    #[test]
    fn golden_vertex_basic() {
        let p = vertex_test_provider();
        let r = vertex_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            vec![],
        );
        assert_vertex_json_snapshot!(
            "vertex_basic",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn vertex_transport_builds_projected_request_with_vertex_url_and_preserves_body() {
        let state = VertexTransportState::new("test-project", "us-central1", GcpAuth::ApplicationDefault, false);
        let transport = ProviderTransport::Vertex(VertexTransport { inner: state });
        let compat = ProviderCompat::anthropic_defaults();
        let body = json!({
            "anthropic_version": "vertex-2023-10-16",
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": true
        });
        let tool_wire_shape = ResolvedToolWireShape::AnthropicInputSchema;

        let request = transport
            .build_projected_request("claude-test-model", body.clone(), &compat, tool_wire_shape)
            .expect("vertex projected request should build");

        assert_eq!(
            request.url,
            "https://us-central1-aiplatform.googleapis.com/v1/projects/test-project/locations/us-central1/publishers/anthropic/models/claude-test-model:streamRawPredict"
        );
        assert!(request.headers.is_empty());
        assert_eq!(request.body, body);
        assert!(request.body_bytes.is_none());
        assert_eq!(request.tool_wire_shape, tool_wire_shape);
    }

    #[tokio::test]
    async fn vertex_transport_send_maps_tool_shape_mismatch_to_actionable_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/streamRawPredict"))
            .and(header("authorization", "Bearer cached-token"))
            .and(header("content-type", "application/json"))
            .respond_with(
                ResponseTemplate::new(400).set_body_string("invalid_request_error: body.tools[0].function is missing"),
            )
            .mount(&server)
            .await;

        let state = VertexTransportState::new("test-project", "us-central1", GcpAuth::ApplicationDefault, false);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_secs();
        *state
            .cached_token
            .lock()
            .expect("token cache lock should not be poisoned") = Some(CachedToken {
            token: "cached-token".to_string(),
            expires_at: now + 3600,
        });
        let transport = ProviderTransport::Vertex(VertexTransport { inner: state });
        let request = ProjectedHttpRequest {
            url: format!("{}/streamRawPredict", server.uri()),
            headers: HeaderMap::new(),
            body: json!({"messages": []}),
            body_bytes: None,
            tool_wire_shape: ResolvedToolWireShape::AnthropicInputSchema,
        };

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
}
