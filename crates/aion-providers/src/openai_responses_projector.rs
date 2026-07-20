use std::collections::HashSet;

use aion_config::compat::ProviderCompat;
use aion_types::llm::LlmRequest;
use aion_types::message::{ContentBlock, Message, Role};
use serde_json::{Value, json};

use crate::openai_messages::generate_call_id;
use crate::openai_responses::PROVIDER_ITEM_OWNER;
use crate::projector::{ProjectionError, ResolvedToolWireShape, WireProvider, preflight_projected_body, project_tools};

pub(crate) struct OpenAiResponsesProjector;

impl OpenAiResponsesProjector {
    pub(crate) fn project(request: &LlmRequest, compat: &ProviderCompat) -> Result<Value, ProjectionError> {
        let max_tokens = request
            .max_tokens
            .or_else(|| compat.default_max_tokens_for_model(&request.model));
        let input = project_input(&request.messages, compat);

        let mut body = json!({
            "model": request.model,
            "instructions": request.system,
            "input": input,
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"]
        });

        if let Some(max_tokens) = max_tokens {
            body["max_output_tokens"] = json!(max_tokens);
        }

        let mut tool_count = 0;
        if !request.tools.is_empty() && compat.emit_tools() {
            let tools = project_tools(&request.tools, ResolvedToolWireShape::OpenAiFunction)
                .into_iter()
                .filter_map(flatten_function_tool)
                .collect::<Vec<_>>();
            tool_count = tools.len();
            body["tools"] = json!(tools);
        } else if !request.tools.is_empty() {
            tracing::warn!(
                target: "aion_providers",
                "OpenAI Responses outgoing tools omitted because compat.emit_tools is disabled"
            );
        }

        if let Some(effort) = &request.reasoning_effort {
            if compat.supports_effort() {
                body["reasoning"] = json!({ "effort": effort });
            } else {
                tracing::warn!(
                    target: "aion_providers",
                    "OpenAI Responses reasoning effort omitted because compat.supports_effort is disabled"
                );
            }
        }

        preflight_projected_body(WireProvider::OpenAi, &body, tool_count, compat)?;
        Ok(body)
    }
}

fn flatten_function_tool(tool: Value) -> Option<Value> {
    let function = tool.get("function")?.as_object()?;
    let mut flattened = function.clone();
    flattened.insert("type".to_string(), json!("function"));
    // Chat Completions tools are non-strict by default. Preserve that contract
    // when the same ToolDef is projected to Responses.
    flattened.insert("strict".to_string(), json!(false));
    Some(Value::Object(flattened))
}

fn project_input(messages: &[Message], compat: &ProviderCompat) -> Vec<Value> {
    let tool_use_ids = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, .. } if !id.is_empty() => Some(id.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let tool_result_ids = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    let mut input = Vec::new();
    for message in messages {
        project_provider_items(message, &mut input);

        match message.role {
            Role::User | Role::Tool => project_user_message(message, compat, &tool_use_ids, &mut input),
            Role::Assistant => project_assistant_message(message, compat, &tool_result_ids, &mut input),
            Role::System => {}
        }
    }
    input
}

fn project_provider_items(message: &Message, input: &mut Vec<Value>) {
    for block in &message.content {
        if let ContentBlock::ProviderItem { provider, item } = block
            && provider == PROVIDER_ITEM_OWNER
        {
            input.push(item.clone());
        }
    }
}

fn project_user_message(
    message: &Message,
    compat: &ProviderCompat,
    tool_use_ids: &HashSet<String>,
    input: &mut Vec<Value>,
) {
    let mut content = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text } => {
                let text = strip_patterns(text, compat);
                if !text.is_empty() {
                    content.push(json!({ "type": "input_text", "text": text }));
                }
            }
            ContentBlock::Image { image_url } => {
                if let Err(error) = image_url.validate() {
                    tracing::warn!(
                        target: "aion_providers",
                        error = %error,
                        "skipping invalid image block in OpenAI Responses projection"
                    );
                    continue;
                }
                content.push(json!({
                    "type": "input_image",
                    "image_url": image_url.url
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content: output,
                ..
            } => {
                if compat.clean_orphan_tool_results() && !tool_use_ids.contains(tool_use_id) {
                    tracing::warn!(
                        target: "aion_providers",
                        tool_call_id = %tool_use_id,
                        reason = "orphan_result",
                        "dropped orphan function_call_output in OpenAI Responses request"
                    );
                    continue;
                }
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": tool_use_id,
                    "output": output
                }));
            }
            ContentBlock::ToolUse { .. } | ContentBlock::Thinking { .. } | ContentBlock::ProviderItem { .. } => {}
        }
    }

    if !content.is_empty() {
        input.push(json!({ "role": "user", "content": content }));
    }
}

fn project_assistant_message(
    message: &Message,
    compat: &ProviderCompat,
    tool_result_ids: &HashSet<String>,
    input: &mut Vec<Value>,
) {
    let text = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let text = strip_patterns(&text, compat);
    if !text.is_empty() {
        input.push(json!({ "role": "assistant", "content": text }));
    }

    for block in &message.content {
        let ContentBlock::ToolUse {
            id,
            name,
            input: arguments,
            extra,
        } = block
        else {
            continue;
        };

        if compat.sanitize_malformed_tool_calls() && name.is_empty() {
            tracing::warn!(
                target: "aion_providers",
                tool_call_id = %id,
                reason = "empty_name",
                "dropped malformed function_call in OpenAI Responses request"
            );
            continue;
        }
        if compat.clean_orphan_tool_calls() && !tool_result_ids.contains(id) {
            tracing::warn!(
                target: "aion_providers",
                tool_call_id = %id,
                reason = "orphan_call",
                "dropped orphan function_call in OpenAI Responses request"
            );
            continue;
        }

        if let Some(item) = raw_function_call(extra) {
            input.push(item.clone());
            continue;
        }

        let call_id = if id.is_empty() && compat.auto_tool_id() {
            generate_call_id()
        } else {
            id.clone()
        };
        input.push(json!({
            "type": "function_call",
            "call_id": call_id,
            "name": name,
            "arguments": serde_json::to_string(arguments).unwrap_or_default()
        }));
    }
}

fn raw_function_call(extra: &Option<Value>) -> Option<&Value> {
    extra
        .as_ref()?
        .get(PROVIDER_ITEM_OWNER)?
        .get("function_call")
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
}

fn strip_patterns(text: &str, compat: &ProviderCompat) -> String {
    let mut result = text.to_string();
    if let Some(patterns) = &compat.messages.strip_patterns {
        for pattern in patterns {
            result = result.replace(pattern, "");
        }
    }
    result
}

#[cfg(test)]
#[path = "openai_responses_projector_test.rs"]
mod openai_responses_projector_test;
