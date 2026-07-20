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
    #[error("{provider} request body is {bytes} bytes, exceeding configured limit {max_bytes} bytes")]
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

        let max_tokens = request
            .max_tokens
            .or_else(|| compat.default_max_tokens_for_model(&request.model));

        let mut body = json!({
            "system": system,
            "messages": anthropic_shared::build_messages(&request.messages, compat)
        });
        if let Some(max_tokens) = max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }

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
            if let Some(last) = tools
                .last_mut()
                .filter(|_| params.cache_enabled && tool_wire_shape == ResolvedToolWireShape::AnthropicInputSchema)
            {
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

    pub(crate) fn project(request: &LlmRequest, compat: &ProviderCompat) -> Result<Value, ProjectionError> {
        let max_tokens_field = compat.max_tokens_field();
        let max_tokens = request
            .max_tokens
            .or_else(|| compat.default_max_tokens_for_model(&request.model));

        let mut body = json!({
            "model": request.model,
            "messages": openai_messages::build_messages(
                &request.messages,
                &request.system,
                compat,
            ),
            "stream": true
        });
        if let Some(max_tokens) = max_tokens {
            body[max_tokens_field] = json!(max_tokens);
        }

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

        if let Some(thinking) = &request.thinking {
            match thinking {
                ThinkingConfig::Enabled { .. } => {
                    body["thinking"] = json!({ "type": "enabled" });
                }
                ThinkingConfig::Disabled => {
                    body["thinking"] = json!({ "type": "disabled" });
                }
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

fn resolve_tool_wire_shape(compat: &ProviderCompat, native: ResolvedToolWireShape) -> ResolvedToolWireShape {
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
        (tool.description.clone(), legalize_json_schema(&tool.input_schema))
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
    let expected_shape =
        if lower.contains("body.tools[0].function") && (lower.contains("missing") || lower.contains("required")) {
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

pub(crate) fn preflight_projected_body(
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
#[path = "projector_test.rs"]
mod projector_test;
