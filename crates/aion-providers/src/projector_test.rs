use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use aion_config::schema::legalize_json_schema;
    use aion_types::message::{ContentBlock, Message, Role};
    use aion_types::tool::ToolDef;

    fn test_request(tools: Vec<ToolDef>, thinking: Option<ThinkingConfig>) -> LlmRequest {
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
            thinking,
            reasoning_effort: None,
        }
    }

    fn test_tools() -> Vec<ToolDef> {
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

    fn numbered_tools(count: usize) -> Vec<ToolDef> {
        (0..count)
            .map(|index| ToolDef {
                name: format!("tool_{index}"),
                description: format!("Tool {index}"),
                input_schema: json!({"type":"object","properties":{}}),
                deferred: false,
            })
            .collect()
    }

    #[test]
    fn test_build_tools_deferred_has_empty_parameters() {
        let tools = vec![
            ToolDef {
                name: "Read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                deferred: false,
            },
            ToolDef {
                name: "SpawnTool".into(),
                description: "Spawn sub-agents".into(),
                input_schema: json!({"type": "object", "properties": {"agents": {"type": "array"}}}),
                deferred: true,
            },
        ];
        let result = project_tools(&tools, ResolvedToolWireShape::OpenAiFunction);

        let read_params = &result[0]["function"]["parameters"];
        assert!(read_params["properties"].get("path").is_some());

        let spawn_params = &result[1]["function"]["parameters"];
        assert!(spawn_params["properties"].as_object().unwrap().is_empty());
        let spawn_desc = result[1]["function"]["description"].as_str().unwrap();
        assert!(spawn_desc.contains("ToolSearch"));
    }

    #[test]
    fn test_build_tools_legalizes_null_and_empty_parameters() {
        let tools = vec![
            ToolDef {
                name: "NullSchema".into(),
                description: "Null schema".into(),
                input_schema: Value::Null,
                deferred: false,
            },
            ToolDef {
                name: "EmptySchema".into(),
                description: "Empty schema".into(),
                input_schema: json!({}),
                deferred: false,
            },
            ToolDef {
                name: "StringSchema".into(),
                description: "String schema".into(),
                input_schema: json!("raw"),
                deferred: false,
            },
            ToolDef {
                name: "StringRootType".into(),
                description: "String root type".into(),
                input_schema: json!({"type": "string"}),
                deferred: false,
            },
        ];
        let result = project_tools(&tools, ResolvedToolWireShape::OpenAiFunction);

        for tool in result {
            assert_eq!(
                tool["function"]["parameters"],
                json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "properties": {}
                })
            );
        }
    }

    #[test]
    fn test_anthropic_wire_params_shape_anthropic_body() {
        let request = test_request(test_tools(), Some(ThinkingConfig::Enabled { budget_tokens: 4096 }));

        let body = AnthropicWireProjector::project(
            &request,
            &ProviderCompat::anthropic_defaults(),
            WireParams {
                provider: WireProvider::Anthropic,
                anthropic_version: None,
                include_model_in_body: true,
                include_stream: true,
                cache_enabled: true,
                sanitize_schema: false,
            },
        )
        .expect("request body projection should succeed");

        assert_eq!(
            body,
            json!({
                "model": "test-model",
                "max_tokens": 8192,
                "system": [{
                    "type": "text",
                    "text": "You are a test assistant.",
                    "cache_control": { "type": "ephemeral" }
                }],
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "Hello"}]
                }],
                "stream": true,
                "tools": [
                    {
                        "name": "read",
                        "description": "Read",
                        "input_schema": {
                            "$schema": "https://json-schema.org/draft/2020-12/schema",
                            "type":"object",
                            "properties":{"path":{"type":"string"}},
                            "required":["path"]
                        }
                    },
                    {
                        "name": "list",
                        "description": "List",
                        "input_schema": {
                            "$schema": "https://json-schema.org/draft/2020-12/schema",
                            "type":"object",
                            "properties":{}
                        },
                        "cache_control": { "type": "ephemeral" }
                    }
                ],
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 4096
                }
            })
        );
    }

    #[test]
    fn test_anthropic_wire_params_shape_bedrock_body() {
        let request = test_request(test_tools(), None);

        let body = AnthropicWireProjector::project(
            &request,
            &ProviderCompat::bedrock_defaults(),
            WireParams {
                provider: WireProvider::Bedrock,
                anthropic_version: Some("bedrock-2023-05-31"),
                include_model_in_body: false,
                include_stream: false,
                cache_enabled: false,
                sanitize_schema: false,
            },
        )
        .expect("request body projection should succeed");

        assert_eq!(
            body,
            json!({
                "anthropic_version": "bedrock-2023-05-31",
                "max_tokens": 8192,
                "system": "You are a test assistant.",
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "Hello"}]
                }],
                "tools": [
                    {
                        "name": "read",
                        "description": "Read",
                        "input_schema": {
                            "$schema": "https://json-schema.org/draft/2020-12/schema",
                            "type":"object",
                            "properties":{"path":{"type":"string"}},
                            "required":["path"]
                        }
                    },
                    {
                        "name": "list",
                        "description": "List",
                        "input_schema": {
                            "$schema": "https://json-schema.org/draft/2020-12/schema",
                            "type":"object",
                            "properties":{}
                        }
                    }
                ]
            })
        );
    }

    #[test]
    fn test_anthropic_wire_params_shape_vertex_body() {
        let request = test_request(vec![], None);

        let body = AnthropicWireProjector::project(
            &request,
            &ProviderCompat::anthropic_defaults(),
            WireParams {
                provider: WireProvider::Vertex,
                anthropic_version: Some("vertex-2023-10-16"),
                include_model_in_body: false,
                include_stream: true,
                cache_enabled: false,
                sanitize_schema: false,
            },
        )
        .expect("request body projection should succeed");

        assert_eq!(
            body,
            json!({
                "anthropic_version": "vertex-2023-10-16",
                "max_tokens": 8192,
                "system": "You are a test assistant.",
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "Hello"}]
                }],
                "stream": true
            })
        );
    }

    #[test]
    fn test_anthropic_wire_projector_sanitizes_schema_only_when_requested() {
        let request = test_request(
            vec![ToolDef {
                name: "read".to_string(),
                description: "Read".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {"path": {"type": ["string", "null"]}},
                    "additionalProperties": false
                }),
                deferred: false,
            }],
            None,
        );
        let compat = ProviderCompat::bedrock_defaults();
        let params = WireParams {
            anthropic_version: Some("bedrock-2023-05-31"),
            provider: WireProvider::Bedrock,
            include_model_in_body: false,
            include_stream: false,
            cache_enabled: false,
            sanitize_schema: false,
        };

        let unsanitized =
            AnthropicWireProjector::project(&request, &compat, params).expect("request body projection should succeed");
        assert_eq!(
            unsanitized["tools"][0]["input_schema"],
            legalize_json_schema(&request.tools[0].input_schema)
        );
        assert_eq!(unsanitized["tools"][0]["input_schema"]["additionalProperties"], false);

        let sanitized = AnthropicWireProjector::project(
            &request,
            &compat,
            WireParams {
                sanitize_schema: true,
                ..params
            },
        )
        .expect("request body projection should succeed");
        assert_eq!(
            sanitized["tools"][0]["input_schema"],
            compat::sanitize_json_schema(&legalize_json_schema(&request.tools[0].input_schema))
        );
        assert!(sanitized["tools"][0]["input_schema"]["additionalProperties"].is_null());
    }

    #[test]
    fn test_bedrock_strict_sanitize_runs_after_universal_legalize() {
        let request = test_request(
            vec![ToolDef {
                name: "empty".to_string(),
                description: "Empty".to_string(),
                input_schema: json!({
                    "additionalProperties": false
                }),
                deferred: false,
            }],
            None,
        );

        let body = AnthropicWireProjector::project(
            &request,
            &ProviderCompat::bedrock_defaults(),
            WireParams {
                provider: WireProvider::Bedrock,
                anthropic_version: Some("bedrock-2023-05-31"),
                include_model_in_body: false,
                include_stream: false,
                cache_enabled: false,
                sanitize_schema: true,
            },
        )
        .expect("request body projection should succeed");

        let schema = &body["tools"][0]["input_schema"];
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["$schema"], "https://json-schema.org/draft/2020-12/schema");
        assert!(schema["properties"].as_object().unwrap().is_empty());
        assert!(schema.get("additionalProperties").is_none());
    }

    #[test]
    fn test_openai_projector_uses_custom_max_tokens_field() {
        let request = test_request(vec![], None);
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.max_tokens_field = Some("max_completion_tokens".to_string());

        let body = OpenAiProjector::project(&request, &compat).expect("request body projection should succeed");

        assert_eq!(body["max_completion_tokens"], 8192);
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn test_openai_projector_returns_success_result() {
        let request = test_request(vec![], None);
        let body = OpenAiProjector::project(&request, &ProviderCompat::openai_defaults())
            .expect("request body projection should succeed");

        assert_eq!(body["model"], "test-model");
    }

    #[test]
    fn test_openai_projector_default_includes_stream_options() {
        let request = test_request(vec![], None);
        let body = OpenAiProjector::project(&request, &ProviderCompat::openai_defaults())
            .expect("request body projection should succeed");

        assert_eq!(body["stream_options"], json!({ "include_usage": true }));
    }

    #[test]
    fn test_openai_projector_omits_stream_options_when_disabled() {
        let request = test_request(vec![], None);
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.include_stream_options = Some(false);

        let body = OpenAiProjector::project(&request, &compat).expect("request body projection should succeed");

        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn test_openai_projector_omits_tools_when_disabled_without_mutating_request() {
        let request = test_request(test_tools(), None);
        let mut compat = ProviderCompat::openai_defaults();
        compat.tools.emit_tools = Some(false);

        let body = OpenAiProjector::project(&request, &compat).expect("request body projection should succeed");

        assert!(body.get("tools").is_none());
        assert_eq!(request.tools.len(), 2);
        assert_eq!(request.tools[0].name, "read");
        assert_eq!(request.tools[1].name, "list");
    }

    #[test]
    fn test_openai_projector_omits_reasoning_effort_when_effort_disabled() {
        let mut request = test_request(vec![], None);
        request.reasoning_effort = Some("medium".to_string());
        let mut compat = ProviderCompat::openai_defaults();
        compat.reasoning.supports_effort = Some(false);

        let body = OpenAiProjector::project(&request, &compat).expect("request body projection should succeed");

        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn test_tool_wire_shape_anthropic_default_emits_input_schema() {
        let request = test_request(test_tools(), None);
        let body = AnthropicWireProjector::project(
            &request,
            &ProviderCompat::anthropic_defaults(),
            WireParams {
                provider: WireProvider::Anthropic,
                anthropic_version: None,
                include_model_in_body: true,
                include_stream: true,
                cache_enabled: false,
                sanitize_schema: false,
            },
        )
        .expect("request body projection should succeed");

        assert_eq!(body["tools"][0]["name"], "read");
        assert!(body["tools"][0].get("input_schema").is_some());
        assert!(body["tools"][0].get("function").is_none());
    }

    #[test]
    fn test_tool_wire_shape_anthropic_override_openai_function() {
        let request = test_request(test_tools(), None);
        let user_compat: ProviderCompat =
            serde_json::from_value(json!({"tool_wire_shape": "openai_function"})).unwrap();
        let compat = ProviderCompat::merge(ProviderCompat::anthropic_defaults(), user_compat);

        let body = AnthropicWireProjector::project(
            &request,
            &compat,
            WireParams {
                provider: WireProvider::Anthropic,
                anthropic_version: None,
                include_model_in_body: true,
                include_stream: true,
                cache_enabled: false,
                sanitize_schema: false,
            },
        )
        .expect("request body projection should succeed");

        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        assert!(body["tools"][0]["function"].get("parameters").is_some());
        assert!(body["tools"][0].get("input_schema").is_none());
    }

    #[test]
    fn test_tool_wire_shape_openai_default_emits_function() {
        let request = test_request(test_tools(), None);
        let body = OpenAiProjector::project(&request, &ProviderCompat::openai_defaults())
            .expect("request body projection should succeed");

        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        assert!(body["tools"][0]["function"].get("parameters").is_some());
        assert!(body["tools"][0].get("input_schema").is_none());
    }

    #[test]
    fn test_tool_wire_shape_openai_override_anthropic_input_schema() {
        let request = test_request(test_tools(), None);
        let user_compat: ProviderCompat =
            serde_json::from_value(json!({"tool_wire_shape": "anthropic_input_schema"})).unwrap();
        let compat = ProviderCompat::merge(ProviderCompat::openai_defaults(), user_compat);

        let body = OpenAiProjector::project(&request, &compat).expect("request body projection should succeed");

        assert_eq!(body["tools"][0]["name"], "read");
        assert!(body["tools"][0].get("input_schema").is_some());
        assert!(body["tools"][0].get("function").is_none());
    }

    #[test]
    fn test_anthropic_projector_returns_success_result() {
        let request = test_request(vec![], None);
        let body = AnthropicWireProjector::project(
            &request,
            &ProviderCompat::anthropic_defaults(),
            WireParams {
                provider: WireProvider::Anthropic,
                anthropic_version: None,
                include_model_in_body: true,
                include_stream: true,
                cache_enabled: false,
                sanitize_schema: false,
            },
        )
        .expect("request body projection should succeed");

        assert_eq!(body["model"], "test-model");
    }

    #[test]
    fn test_preflight_tool_count_limit_rejects_openai_tools() {
        let request = test_request(numbered_tools(513), None);
        let mut compat = ProviderCompat::openai_defaults();
        compat.tools.max_tool_count = Some(512);

        let error =
            OpenAiProjector::project(&request, &compat).expect_err("tool count over the configured limit should fail");

        match error {
            ProjectionError::ToolLimitExceeded { provider, count, max } => {
                assert_eq!(provider, WireProvider::OpenAi);
                assert_eq!(count, 513);
                assert_eq!(max, 512);
            }
            other => panic!("unexpected projection error: {other}"),
        }
    }

    #[test]
    fn test_preflight_request_body_size_limit_rejects_openai_body() {
        let request = test_request(vec![], None);
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.max_request_body_bytes = Some(1);

        let error = OpenAiProjector::project(&request, &compat)
            .expect_err("request body over the configured byte limit should fail");

        match error {
            ProjectionError::BodyLimitExceeded {
                provider,
                bytes,
                max_bytes,
            } => {
                assert_eq!(provider, WireProvider::OpenAi);
                assert!(bytes > 1);
                assert_eq!(max_bytes, 1);
            }
            other => panic!("unexpected projection error: {other}"),
        }
    }
}
