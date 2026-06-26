use aion_config::compat::{self, ProviderCompat};
use aion_types::llm::{LlmRequest, ThinkingConfig};
use serde_json::{Value, json};

use crate::anthropic_shared;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WireParams {
    pub anthropic_version: Option<&'static str>,
    pub include_model_in_body: bool,
    pub include_stream: bool,
    pub cache_enabled: bool,
    pub sanitize_schema: bool,
}

pub(crate) struct AnthropicWireProjector;

impl AnthropicWireProjector {
    pub(crate) fn project(
        request: &LlmRequest,
        compat: &ProviderCompat,
        params: WireParams,
    ) -> Value {
        let system = if params.cache_enabled {
            json!([{
                "type": "text",
                "text": &request.system,
                "cache_control": { "type": "ephemeral" }
            }])
        } else {
            json!(&request.system)
        };

        let mut body = json!({
            "max_tokens": request.max_tokens,
            "system": system,
            "messages": anthropic_shared::build_messages(&request.messages, compat)
        });

        if params.include_model_in_body {
            body["model"] = json!(request.model);
        }

        if let Some(version) = params.anthropic_version {
            body["anthropic_version"] = json!(version);
        }

        if params.include_stream {
            body["stream"] = json!(true);
        }

        if !request.tools.is_empty() {
            let mut tools = anthropic_shared::build_tools(&request.tools);
            if params.sanitize_schema {
                for tool in &mut tools {
                    if let Some(schema) = tool.get("input_schema").cloned() {
                        tool["input_schema"] = compat::sanitize_json_schema(&schema);
                    }
                }
            }
            if let Some(last) = tools.last_mut().filter(|_| params.cache_enabled) {
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

pub(crate) struct OpenAiProjector;

impl OpenAiProjector {
    pub(crate) fn project(request: &LlmRequest, compat: &ProviderCompat) -> Value {
        let max_tokens_field = compat.max_tokens_field();

        let mut body = json!({
            "model": request.model,
            "messages": crate::openai::OpenAIProvider::build_messages(
                &request.messages,
                &request.system,
                compat,
            ),
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        body[max_tokens_field] = json!(request.max_tokens);

        if !request.tools.is_empty() {
            body["tools"] = json!(crate::openai::OpenAIProvider::build_tools(&request.tools));
        }

        if let Some(effort) = &request.reasoning_effort {
            body["reasoning_effort"] = json!(effort);
        }

        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn test_anthropic_wire_params_shape_anthropic_body() {
        let request = test_request(
            test_tools(),
            Some(ThinkingConfig::Enabled {
                budget_tokens: 4096,
            }),
        );

        let body = AnthropicWireProjector::project(
            &request,
            &ProviderCompat::anthropic_defaults(),
            WireParams {
                anthropic_version: None,
                include_model_in_body: true,
                include_stream: true,
                cache_enabled: true,
                sanitize_schema: false,
            },
        );

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
                        "input_schema": {"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}
                    },
                    {
                        "name": "list",
                        "description": "List",
                        "input_schema": {"type":"object","properties":{}},
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
                anthropic_version: Some("bedrock-2023-05-31"),
                include_model_in_body: false,
                include_stream: false,
                cache_enabled: false,
                sanitize_schema: false,
            },
        );

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
                        "input_schema": {"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}
                    },
                    {
                        "name": "list",
                        "description": "List",
                        "input_schema": {"type":"object","properties":{}}
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
                anthropic_version: Some("vertex-2023-10-16"),
                include_model_in_body: false,
                include_stream: true,
                cache_enabled: false,
                sanitize_schema: false,
            },
        );

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
            include_model_in_body: false,
            include_stream: false,
            cache_enabled: false,
            sanitize_schema: false,
        };

        let unsanitized = AnthropicWireProjector::project(&request, &compat, params);
        assert_eq!(
            unsanitized["tools"][0]["input_schema"],
            request.tools[0].input_schema
        );

        let sanitized = AnthropicWireProjector::project(
            &request,
            &compat,
            WireParams {
                sanitize_schema: true,
                ..params
            },
        );
        assert_eq!(
            sanitized["tools"][0]["input_schema"],
            compat::sanitize_json_schema(&request.tools[0].input_schema)
        );
        assert!(sanitized["tools"][0]["input_schema"]["additionalProperties"].is_null());
    }

    #[test]
    fn test_openai_projector_uses_custom_max_tokens_field() {
        let request = test_request(vec![], None);
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.max_tokens_field = Some("max_completion_tokens".to_string());

        let body = OpenAiProjector::project(&request, &compat);

        assert_eq!(body["max_completion_tokens"], 8192);
        assert!(body.get("max_tokens").is_none());
    }
}
