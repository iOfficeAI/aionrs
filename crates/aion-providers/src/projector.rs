use aion_config::compat::{self, ProviderCompat, ToolWireShape};
use aion_config::schema::legalize_json_schema;
use aion_types::llm::{LlmRequest, ThinkingConfig};
use aion_types::tool::{ToolDef, truncate_deferred_description};
use serde_json::{Value, json};
use std::fmt;

use crate::ProviderError;
use crate::{anthropic_shared, openai_messages};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WireProvider {
    OpenAi,
    Anthropic,
    Bedrock,
    Vertex,
}

impl WireProvider {
    fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Bedrock => "bedrock",
            Self::Vertex => "vertex",
        }
    }
}

impl fmt::Display for WireProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProjectionError {
    #[error("{provider} tools count {count} exceeds configured limit {max}")]
    ToolLimitExceeded {
        provider: WireProvider,
        count: usize,
        max: usize,
    },
    #[error(
        "{provider} request body is {bytes} bytes, exceeding configured limit {max_bytes} bytes"
    )]
    BodyLimitExceeded {
        provider: WireProvider,
        bytes: usize,
        max_bytes: usize,
    },
    #[error("{provider} tool schema for {tool_name} is invalid: {reason}")]
    SchemaInvalid {
        provider: WireProvider,
        tool_name: String,
        reason: String,
    },
}

pub(crate) fn projection_to_provider_error(error: ProjectionError) -> ProviderError {
    ProviderError::PromptTooLong(error.to_string())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WireParams {
    pub provider: WireProvider,
    pub anthropic_version: Option<&'static str>,
    pub include_model_in_body: bool,
    pub include_stream: bool,
    pub cache_enabled: bool,
    pub sanitize_schema: bool,
}

pub(crate) struct AnthropicWireProjector;

impl AnthropicWireProjector {
    pub(crate) fn resolved_tool_wire_shape(compat: &ProviderCompat) -> ResolvedToolWireShape {
        resolve_tool_wire_shape(compat, ResolvedToolWireShape::AnthropicInputSchema)
    }

    pub(crate) fn project(
        request: &LlmRequest,
        compat: &ProviderCompat,
        params: WireParams,
    ) -> Result<Value, ProjectionError> {
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

        let mut tool_count = 0;
        if !request.tools.is_empty() {
            let tool_wire_shape = Self::resolved_tool_wire_shape(compat);
            let mut tools = project_tools(&request.tools, tool_wire_shape);
            tool_count = tools.len();
            if params.sanitize_schema {
                for tool in &mut tools {
                    if let Some(schema) = projected_tool_schema_mut(tool, tool_wire_shape) {
                        *schema = compat::sanitize_json_schema(schema);
                    }
                }
            }
            if let Some(last) = tools.last_mut().filter(|_| {
                params.cache_enabled
                    && tool_wire_shape == ResolvedToolWireShape::AnthropicInputSchema
            }) {
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

        preflight_projected_body(params.provider, &body, tool_count, compat)?;

        Ok(body)
    }
}

pub(crate) struct OpenAiProjector;

impl OpenAiProjector {
    pub(crate) fn resolved_tool_wire_shape(compat: &ProviderCompat) -> ResolvedToolWireShape {
        resolve_tool_wire_shape(compat, ResolvedToolWireShape::OpenAiFunction)
    }

    pub(crate) fn project(
        request: &LlmRequest,
        compat: &ProviderCompat,
    ) -> Result<Value, ProjectionError> {
        let max_tokens_field = compat.max_tokens_field();

        let mut body = json!({
            "model": request.model,
            "messages": openai_messages::build_messages(
                &request.messages,
                &request.system,
                compat,
            ),
            "stream": true
        });
        body[max_tokens_field] = json!(request.max_tokens);

        if compat.include_stream_options() {
            body["stream_options"] = json!({ "include_usage": true });
        }

        let mut tool_count = 0;
        if !request.tools.is_empty() && compat.emit_tools() {
            let tools = project_tools(&request.tools, Self::resolved_tool_wire_shape(compat));
            tool_count = tools.len();
            body["tools"] = json!(tools);
        } else if !request.tools.is_empty() {
            tracing::warn!(
                target: "aion_providers",
                "OpenAI-compatible outgoing tools omitted because compat.emit_tools is disabled"
            );
        }

        if let Some(effort) = &request.reasoning_effort {
            if compat.supports_effort() {
                body["reasoning_effort"] = json!(effort);
            } else {
                tracing::warn!(
                    target: "aion_providers",
                    "OpenAI-compatible reasoning_effort omitted because compat.supports_effort is disabled"
                );
            }
        }

        preflight_projected_body(WireProvider::OpenAi, &body, tool_count, compat)?;

        Ok(body)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedToolWireShape {
    OpenAiFunction,
    AnthropicInputSchema,
}

impl ResolvedToolWireShape {
    pub(crate) const fn as_config_value(self) -> &'static str {
        match self {
            Self::OpenAiFunction => "openai_function",
            Self::AnthropicInputSchema => "anthropic_input_schema",
        }
    }
}

fn resolve_tool_wire_shape(
    compat: &ProviderCompat,
    native: ResolvedToolWireShape,
) -> ResolvedToolWireShape {
    match compat.tool_wire_shape() {
        ToolWireShape::Native => native,
        ToolWireShape::OpenAiFunction => ResolvedToolWireShape::OpenAiFunction,
        ToolWireShape::AnthropicInputSchema => ResolvedToolWireShape::AnthropicInputSchema,
    }
}

pub(crate) fn project_tools(tools: &[ToolDef], shape: ResolvedToolWireShape) -> Vec<Value> {
    tools.iter().map(|tool| project_tool(tool, shape)).collect()
}

fn project_tool(tool: &ToolDef, shape: ResolvedToolWireShape) -> Value {
    match shape {
        ResolvedToolWireShape::OpenAiFunction => project_openai_function_tool(tool),
        ResolvedToolWireShape::AnthropicInputSchema => project_anthropic_input_schema_tool(tool),
    }
}

fn project_openai_function_tool(tool: &ToolDef) -> Value {
    let (description, parameters) = tool_description_and_schema(tool);
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": description,
            "parameters": parameters
        }
    })
}

fn project_anthropic_input_schema_tool(tool: &ToolDef) -> Value {
    let (description, input_schema) = tool_description_and_schema(tool);
    json!({
        "name": tool.name,
        "description": description,
        "input_schema": input_schema
    })
}

fn tool_description_and_schema(tool: &ToolDef) -> (String, Value) {
    if tool.deferred {
        let short_desc = truncate_deferred_description(&tool.description);
        (
            format!("(Deferred) {short_desc} — Use ToolSearch to load full schema before calling."),
            legalize_json_schema(&json!({
                "type": "object",
                "properties": {}
            })),
        )
    } else {
        (
            tool.description.clone(),
            legalize_json_schema(&tool.input_schema),
        )
    }
}

fn projected_tool_schema_mut(tool: &mut Value, shape: ResolvedToolWireShape) -> Option<&mut Value> {
    match shape {
        ResolvedToolWireShape::OpenAiFunction => tool
            .get_mut("function")
            .and_then(|function| function.get_mut("parameters")),
        ResolvedToolWireShape::AnthropicInputSchema => tool.get_mut("input_schema"),
    }
}

pub(crate) fn classify_tools_wire_shape_mismatch(
    status: u16,
    body_text: &str,
    configured_shape: ResolvedToolWireShape,
) -> Option<String> {
    if status != 400 {
        return None;
    }

    let lower = body_text.to_ascii_lowercase();
    let expected_shape = if lower.contains("body.tools[0].function")
        && (lower.contains("missing") || lower.contains("required"))
    {
        Some(ResolvedToolWireShape::OpenAiFunction)
    } else if lower.contains("input tag function does not match expected custom") {
        Some(ResolvedToolWireShape::AnthropicInputSchema)
    } else {
        None
    }?;

    Some(format!(
        "tools wire shape mismatch: configured tool_wire_shape resolved to {}; upstream appears to expect {}; upstream error: {}",
        configured_shape.as_config_value(),
        expected_shape.as_config_value(),
        body_text
    ))
}

fn preflight_projected_body(
    provider: WireProvider,
    body: &Value,
    tool_count: usize,
    compat: &ProviderCompat,
) -> Result<(), ProjectionError> {
    if let Some(max) = compat.max_tool_count()
        && tool_count > max
    {
        return Err(ProjectionError::ToolLimitExceeded {
            provider,
            count: tool_count,
            max,
        });
    }

    if let Some(max_bytes) = compat.max_request_body_bytes() {
        let bytes = serde_json::to_vec(body)
            .map_err(|error| ProjectionError::SchemaInvalid {
                provider,
                tool_name: "<request-body>".to_string(),
                reason: error.to_string(),
            })?
            .len();
        if bytes > max_bytes {
            return Err(ProjectionError::BodyLimitExceeded {
                provider,
                bytes,
                max_bytes,
            });
        }
    }

    Ok(())
}

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

        let unsanitized = AnthropicWireProjector::project(&request, &compat, params)
            .expect("request body projection should succeed");
        assert_eq!(
            unsanitized["tools"][0]["input_schema"],
            legalize_json_schema(&request.tools[0].input_schema)
        );
        assert_eq!(
            unsanitized["tools"][0]["input_schema"]["additionalProperties"],
            false
        );

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
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert!(schema["properties"].as_object().unwrap().is_empty());
        assert!(schema.get("additionalProperties").is_none());
    }

    #[test]
    fn test_openai_projector_uses_custom_max_tokens_field() {
        let request = test_request(vec![], None);
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.max_tokens_field = Some("max_completion_tokens".to_string());

        let body = OpenAiProjector::project(&request, &compat)
            .expect("request body projection should succeed");

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

        let body = OpenAiProjector::project(&request, &compat)
            .expect("request body projection should succeed");

        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn test_openai_projector_omits_tools_when_disabled_without_mutating_request() {
        let request = test_request(test_tools(), None);
        let mut compat = ProviderCompat::openai_defaults();
        compat.tools.emit_tools = Some(false);

        let body = OpenAiProjector::project(&request, &compat)
            .expect("request body projection should succeed");

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

        let body = OpenAiProjector::project(&request, &compat)
            .expect("request body projection should succeed");

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

        let body = OpenAiProjector::project(&request, &compat)
            .expect("request body projection should succeed");

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

        let error = OpenAiProjector::project(&request, &compat)
            .expect_err("tool count over the configured limit should fail");

        match error {
            ProjectionError::ToolLimitExceeded {
                provider,
                count,
                max,
            } => {
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
