// Shared Anthropic message/tool building and SSE parsing logic.
// Used by AnthropicProvider, BedrockProvider, and VertexProvider.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};

use aion_types::llm::LlmEvent;
use aion_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use aion_types::tool::ToolDef;

use crate::projector::{ResolvedToolWireShape, project_tools};
use crate::tool_call_sanitize::{DroppedToolCallReason, format_dropped_tool_call};
use aion_config::compat::ProviderCompat;

/// Convert internal Message format to Anthropic API message format.
/// Compat flags control merging and alternation behavior.
pub fn build_messages(messages: &[Message], compat: &ProviderCompat) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();
    let sanitize = compat.sanitize_malformed_tool_calls();
    let auto_tool_id = compat.auto_tool_id();
    let clean_orphan_tool_results = compat.clean_orphan_tool_results();
    let mut dropped_ids: HashMap<String, VecDeque<DroppedToolCallReason>> = HashMap::new();
    let mut available_tool_use_ids: HashSet<String> = HashSet::new();
    let mut generated_tool_use_ids: HashMap<String, VecDeque<String>> = HashMap::new();

    for msg in messages {
        let role_str = match msg.role {
            Role::User | Role::Tool => "user",
            Role::Assistant => "assistant",
            Role::System => continue, // system is top-level in Anthropic
        };

        let mut content: Vec<Value> = Vec::new();
        let mut empty_message_placeholder: Option<&'static str> = None;
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    let mut text = text.clone();
                    if let Some(patterns) = &compat.messages.strip_patterns {
                        for pattern in patterns {
                            text = text.replace(pattern, "");
                        }
                    }
                    content.push(json!({
                        "type": "text",
                        "text": text
                    }));
                }
                ContentBlock::ToolUse { id, name, input, .. } => {
                    if sanitize && name.is_empty() {
                        let reason = DroppedToolCallReason::EmptyName;
                        dropped_ids.entry(id.clone()).or_default().push_back(reason);
                        content.push(json!({
                            "type": "text",
                            "text": format_dropped_tool_call(reason, input)
                        }));
                        tracing::warn!(
                            target: "aion_providers",
                            tool_call_id = %id,
                            reason = reason.log_reason(),
                            "downgraded malformed tool_call to text in outgoing request"
                        );
                        continue;
                    }

                    if sanitize && id.is_empty() && !auto_tool_id {
                        let reason = DroppedToolCallReason::EmptyId;
                        dropped_ids.entry(id.clone()).or_default().push_back(reason);
                        content.push(json!({
                            "type": "text",
                            "text": format_dropped_tool_call(reason, input)
                        }));
                        tracing::warn!(
                            target: "aion_providers",
                            tool_call_id = %id,
                            reason = reason.log_reason(),
                            "downgraded malformed tool_call to text in outgoing request"
                        );
                        continue;
                    }

                    let tool_id = if id.is_empty() && auto_tool_id {
                        generate_tool_id()
                    } else {
                        id.clone()
                    };
                    if id.is_empty() && auto_tool_id {
                        generated_tool_use_ids
                            .entry(id.clone())
                            .or_default()
                            .push_back(tool_id.clone());
                    }
                    available_tool_use_ids.insert(tool_id.clone());
                    content.push(json!({
                        "type": "tool_use",
                        "id": tool_id,
                        "name": name,
                        "input": input
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content: result_content,
                    is_error,
                } => {
                    if let Some(reasons) = dropped_ids.get_mut(tool_use_id)
                        && let Some(reason) = reasons.pop_front()
                    {
                        empty_message_placeholder.get_or_insert(reason.short_placeholder());
                        continue;
                    }

                    let projected_tool_use_id = generated_tool_use_ids
                        .get_mut(tool_use_id)
                        .and_then(VecDeque::pop_front)
                        .unwrap_or_else(|| tool_use_id.clone());

                    if clean_orphan_tool_results && !available_tool_use_ids.contains(&projected_tool_use_id) {
                        empty_message_placeholder.get_or_insert("[tool call skipped: malformed (orphan tool result).]");
                        tracing::warn!(
                            target: "aion_providers",
                            tool_call_id = %tool_use_id,
                            reason = "orphan_result",
                            "dropped orphan tool_result in outgoing request"
                        );
                        continue;
                    }

                    content.push(json!({
                        "type": "tool_result",
                        "tool_use_id": projected_tool_use_id,
                        "content": result_content,
                        "is_error": is_error
                    }));
                }
                ContentBlock::Thinking { thinking, signature } => {
                    let mut value = json!({
                        "type": "thinking",
                        "thinking": thinking
                    });
                    if let Some(signature) = signature {
                        value["signature"] = json!(signature);
                    }
                    content.push(value);
                }
                ContentBlock::Image { image_url } => {
                    if let Err(e) = image_url.validate() {
                        tracing::warn!(
                            target: "aion_providers",
                            error = %e,
                            "skipping invalid image block in Anthropic projection"
                        );
                        continue;
                    }
                    content.push(json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": get_media_type_from_data_uri(&image_url.url),
                            "data": get_base64_data_from_data_uri(&image_url.url)
                        }
                    }));
                }
            }
        }

        if content.is_empty()
            && let Some(placeholder) = empty_message_placeholder
        {
            content.push(json!({
                "type": "text",
                "text": placeholder
            }));
        }

        // Merge consecutive messages with the same role (if enabled)
        if compat.merge_same_role()
            && let Some(last) = result.last_mut()
            && last["role"].as_str() == Some(role_str)
            && let Some(arr) = last["content"].as_array_mut()
        {
            arr.extend(content);
            continue;
        }

        result.push(json!({
            "role": role_str,
            "content": content
        }));
    }

    // Ensure user/assistant alternation (if enabled)
    if compat.ensure_alternation() {
        ensure_message_alternation(&mut result);
    }

    result
}

/// Insert filler messages to ensure strict user/assistant alternation.
fn ensure_message_alternation(messages: &mut Vec<Value>) {
    if messages.is_empty() {
        return;
    }

    // If first message is assistant, prepend a user filler
    if messages[0]["role"].as_str() == Some("assistant") {
        messages.insert(
            0,
            json!({
                "role": "user",
                "content": [{"type": "text", "text": "."}]
            }),
        );
    }

    // Walk through and insert fillers where alternation is broken
    let mut i = 1;
    while i < messages.len() {
        let prev_role = messages[i - 1]["role"].as_str().unwrap_or("");
        let curr_role = messages[i]["role"].as_str().unwrap_or("");
        if prev_role == curr_role {
            let filler_role = if curr_role == "user" { "assistant" } else { "user" };
            messages.insert(
                i,
                json!({
                    "role": filler_role,
                    "content": [{"type": "text", "text": "."}]
                }),
            );
            i += 1; // skip the filler we just inserted
        }
        i += 1;
    }
}

/// Generate a unique tool ID when missing
fn generate_tool_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let rand: u32 = (ts as u32).wrapping_mul(2654435761); // simple hash
    format!("toolu_{:x}_{:08x}", ts, rand)
}

/// Convert internal ToolDef format to Anthropic API tool format.
/// Deferred tools emit a minimal schema to reduce input token usage;
/// the caller must invoke ToolSearch to retrieve the full schema.
pub fn build_tools(tools: &[ToolDef]) -> Vec<Value> {
    project_tools(tools, ResolvedToolWireShape::AnthropicInputSchema)
}

/// State machine for accumulating SSE content blocks
pub struct StreamState {
    /// Current block type being accumulated
    pub current_block_type: Option<String>,
    /// Accumulated tool input JSON fragments
    pub tool_input_json: String,
    /// Tool use ID for current block
    pub tool_id: String,
    /// Tool name for current block
    pub tool_name: String,
    /// Input tokens from message_start
    pub input_tokens: u64,
    /// Output tokens accumulated
    pub output_tokens: u64,
    /// Cache creation tokens (prompt caching)
    pub cache_creation_tokens: u64,
    /// Cache read tokens (prompt caching)
    pub cache_read_tokens: u64,
}

impl Default for StreamState {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamState {
    pub fn new() -> Self {
        Self {
            current_block_type: None,
            tool_input_json: String::new(),
            tool_id: String::new(),
            tool_name: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        }
    }
}

/// Parse a single SSE data payload into zero or more LlmEvents
pub fn parse_sse_data(event_type: &str, data: &str, state: &mut StreamState) -> Vec<LlmEvent> {
    let mut events = Vec::new();

    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return events,
    };

    match event_type {
        "message_start" => {
            if let Some(usage) = json.get("message").and_then(|m| m.get("usage")) {
                state.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0);
                state.cache_creation_tokens = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                state.cache_read_tokens = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
            }
        }

        "content_block_start" => {
            let block = &json["content_block"];
            let block_type = block["type"].as_str().unwrap_or("");
            state.current_block_type = Some(block_type.to_string());

            if block_type == "tool_use" {
                state.tool_id = block["id"].as_str().unwrap_or("").to_string();
                state.tool_name = block["name"].as_str().unwrap_or("").to_string();
                state.tool_input_json.clear();
            }
        }

        "content_block_delta" => {
            let delta = &json["delta"];
            let delta_type = delta["type"].as_str().unwrap_or("");

            match delta_type {
                "text_delta" => {
                    if let Some(text) = delta["text"].as_str() {
                        events.push(LlmEvent::TextDelta(text.to_string()));
                    }
                }
                "input_json_delta" => {
                    if let Some(partial) = delta["partial_json"].as_str() {
                        state.tool_input_json.push_str(partial);
                    }
                }
                "thinking_delta" => {
                    if let Some(thinking) = delta["thinking"].as_str() {
                        events.push(LlmEvent::ThinkingDelta(thinking.to_string()));
                    }
                }
                "signature_delta" => {
                    if let Some(signature) = delta["signature"].as_str() {
                        events.push(LlmEvent::ThinkingSignature(signature.to_string()));
                    }
                }
                _ => {}
            }
        }

        "content_block_stop" => {
            if state.current_block_type.as_deref() == Some("tool_use") {
                let input: Value =
                    serde_json::from_str(&state.tool_input_json).unwrap_or(Value::Object(serde_json::Map::new()));
                if state.tool_name.is_empty() {
                    tracing::warn!(
                        target: "aion_providers",
                        tool_call_id = %state.tool_id,
                        "provider emitted tool_call with empty function name; recorded to history as-is"
                    );
                }
                events.push(LlmEvent::ToolUse {
                    id: state.tool_id.clone(),
                    name: state.tool_name.clone(),
                    input,
                    extra: None,
                });
                state.tool_input_json.clear();
            }
            state.current_block_type = None;
        }

        "message_delta" => {
            let delta = &json["delta"];
            let stop_reason = match delta["stop_reason"].as_str() {
                Some("end_turn") => StopReason::EndTurn,
                Some("tool_use") => StopReason::ToolUse,
                Some("max_tokens") => StopReason::MaxTokens,
                _ => StopReason::EndTurn,
            };

            if let Some(usage) = json.get("usage") {
                state.output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
            }

            events.push(LlmEvent::Done {
                stop_reason,
                usage: TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_creation_tokens: state.cache_creation_tokens,
                    cache_read_tokens: state.cache_read_tokens,
                },
            });
        }

        "message_stop" => {
            // Stream complete, no action needed
        }

        "error" => {
            let msg = json["error"]["message"].as_str().unwrap_or("Unknown API error");
            events.push(LlmEvent::Error(msg.to_string()));
        }

        _ => {}
    }

    events
}

/// Extract media type from a data URI (e.g., "data:image/png;base64,..." -> "image/png")
fn get_media_type_from_data_uri(data_uri: &str) -> &str {
    if let Some(rest) = data_uri.strip_prefix("data:")
        && let Some(semi_pos) = rest.find(';')
    {
        return &rest[..semi_pos];
    }
    "application/octet-stream"
}

/// Extract base64 data from a data URI (e.g., "data:image/png;base64,abc123" -> "abc123")
fn get_base64_data_from_data_uri(data_uri: &str) -> &str {
    if let Some(comma_pos) = data_uri.find(',') {
        return &data_uri[comma_pos + 1..];
    }
    data_uri
}

#[cfg(test)]
#[path = "anthropic_shared_test.rs"]
mod anthropic_shared_test;
