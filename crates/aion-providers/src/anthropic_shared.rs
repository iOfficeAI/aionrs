// Shared Anthropic message/tool building and SSE parsing logic.
// Used by AnthropicProvider, BedrockProvider, and VertexProvider.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use tokio::sync::mpsc;

use aion_types::llm::LlmEvent;
use aion_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use aion_types::tool::{ToolDef, truncate_deferred_description};

use crate::error::ProviderError;
use crate::framing::SseBlockFramer;
use crate::parser::{AnthropicParser, ResponseParser};
pub(crate) use crate::stream_runner::StreamOutcome;
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
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
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

                    if clean_orphan_tool_results
                        && !available_tool_use_ids.contains(&projected_tool_use_id)
                    {
                        empty_message_placeholder
                            .get_or_insert("[tool call skipped: malformed (orphan tool result).]");
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
                ContentBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    let mut value = json!({
                        "type": "thinking",
                        "thinking": thinking
                    });
                    if let Some(signature) = signature {
                        value["signature"] = json!(signature);
                    }
                    content.push(value);
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
            let filler_role = if curr_role == "user" {
                "assistant"
            } else {
                "user"
            };
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
    tools
        .iter()
        .map(|t| {
            if t.deferred {
                let short_desc = truncate_deferred_description(&t.description);
                json!({
                    "name": t.name,
                    "description": format!(
                        "(Deferred) {short_desc} — Use ToolSearch to load full schema before calling."
                    ),
                    "input_schema": {
                        "type": "object",
                        "properties": {}
                    }
                })
            } else {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema
                })
            }
        })
        .collect()
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

/// Process the SSE stream from an Anthropic-compatible API
pub(crate) async fn process_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
) -> StreamOutcome {
    use futures::StreamExt;

    let parser = AnthropicParser;
    let mut state = parser.new_state();
    let mut framer = SseBlockFramer::default();
    let mut stream = response.bytes_stream();
    let mut emitted_content = false;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let err = ProviderError::Connection(e.to_string());
                return if emitted_content {
                    StreamOutcome::FailedPartial(err)
                } else {
                    StreamOutcome::FailedEmpty(err)
                };
            }
        };
        let text = String::from_utf8_lossy(&chunk);
        for frame in framer.push_text(&text) {
            tracing::debug!(target: "aion_providers", chunk = %frame.data, "sse chunk received");
            let events = parser.parse_frame(&frame, &mut state);
            for event in events {
                if matches!(
                    event,
                    LlmEvent::TextDelta(_)
                        | LlmEvent::ThinkingDelta(_)
                        | LlmEvent::ThinkingSignature(_)
                        | LlmEvent::ToolUse { .. }
                ) {
                    emitted_content = true;
                }
                if tx.send(event).await.is_err() {
                    return StreamOutcome::Ok; // receiver dropped
                }
            }
        }
    }

    for event in parser.finish(&mut state) {
        if tx.send(event).await.is_err() {
            return StreamOutcome::Ok;
        }
    }

    StreamOutcome::Ok
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
                state.cache_creation_tokens =
                    usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
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
                let input: Value = serde_json::from_str(&state.tool_input_json)
                    .unwrap_or(Value::Object(serde_json::Map::new()));
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
            let msg = json["error"]["message"]
                .as_str()
                .unwrap_or("Unknown API error");
            events.push(LlmEvent::Error(msg.to_string()));
        }

        _ => {}
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    use aion_config::compat::{MessageCompat, ToolCompat};
    use aion_types::tool::ToolDef;
    use serde_json::json;

    /// Compat with merge but no alternation — matches pre-compat behavior
    fn default_compat() -> ProviderCompat {
        ProviderCompat {
            messages: MessageCompat {
                merge_same_role: Some(true),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn anthropic_compat() -> ProviderCompat {
        ProviderCompat::anthropic_defaults()
    }

    // --- build_messages tests ---

    #[test]
    fn test_build_messages_text_only() {
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user");
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Hello");
    }

    #[test]
    fn test_build_messages_with_tool_use() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                input: json!({"cmd": "ls"}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "assistant");
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["id"], "call_1");
        assert_eq!(content[0]["name"], "bash");
        assert_eq!(content[0]["input"]["cmd"], "ls");
    }

    #[test]
    fn test_build_messages_with_tool_result() {
        let messages = vec![Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "file list".to_string(),
                is_error: false,
            }],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user"); // Tool maps to "user"
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call_1");
        assert_eq!(content[0]["content"], "file list");
        assert_eq!(content[0]["is_error"], false);
    }

    #[test]
    fn test_build_messages_with_thinking() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Thinking {
                thinking: "Let me think...".to_string(),
                signature: None,
            }],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "assistant");
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "Let me think...");
    }

    #[test]
    fn test_build_messages_with_thinking_signature() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Thinking {
                thinking: "Let me think...".to_string(),
                signature: Some("sig-123".to_string()),
            }],
        )];

        let result = build_messages(&messages, &default_compat());

        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "Let me think...");
        assert_eq!(content[0]["signature"], "sig-123");
    }

    // --- compat-driven behavior tests ---

    #[test]
    fn test_ensure_alternation_inserts_user_filler_before_assistant() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Text { text: "hi".into() }],
        )];
        let compat = ProviderCompat {
            messages: MessageCompat {
                ensure_alternation: Some(true),
                merge_same_role: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["role"], "user");
        assert_eq!(result[1]["role"], "assistant");
    }

    #[test]
    fn test_ensure_alternation_disabled_no_filler() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Text { text: "hi".into() }],
        )];
        let compat = ProviderCompat {
            messages: MessageCompat {
                ensure_alternation: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "assistant");
    }

    #[test]
    fn test_merge_same_role_enabled_merges_consecutive_user() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "a".into() }]),
            Message::new(Role::User, vec![ContentBlock::Text { text: "b".into() }]),
        ];
        let compat = ProviderCompat {
            messages: MessageCompat {
                merge_same_role: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 1);
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
    }

    #[test]
    fn test_merge_same_role_disabled_keeps_separate() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "a".into() }]),
            Message::new(Role::User, vec![ContentBlock::Text { text: "b".into() }]),
        ];
        let compat = ProviderCompat {
            messages: MessageCompat {
                merge_same_role: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_auto_tool_id_generates_id_when_empty() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: String::new(),
                name: "bash".into(),
                input: json!({}),
                extra: None,
            }],
        )];
        let compat = ProviderCompat {
            tools: ToolCompat {
                auto_tool_id: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        let content = result[0]["content"].as_array().unwrap();
        let id = content[0]["id"].as_str().unwrap();
        assert!(id.starts_with("toolu_"));
    }

    #[test]
    fn test_auto_tool_id_preserves_existing_id() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "existing_id".into(),
                name: "bash".into(),
                input: json!({}),
                extra: None,
            }],
        )];
        let compat = ProviderCompat {
            tools: ToolCompat {
                auto_tool_id: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["id"], "existing_id");
    }

    // F1-2
    #[test]
    fn test_anthropic_empty_name_downgraded_no_orphan() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Text {
                        text: "writing".into(),
                    },
                    ContentBlock::ToolUse {
                        id: "call_x".into(),
                        name: "".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_x".into(),
                    content: "Unknown tool: ".into(),
                    is_error: true,
                }],
            ),
        ];
        let result = build_messages(&messages, &anthropic_compat());
        let any_empty = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_use" && b["name"] == "");
        assert!(!any_empty);
        let any_orphan = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "call_x");
        assert!(!any_orphan);
        let any_text = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| {
                b["type"] == "text"
                    && b["text"]
                        .as_str()
                        .unwrap_or("")
                        .contains("[tool call skipped:")
            });
        assert!(any_text);
    }

    // F1-4
    #[test]
    fn test_anthropic_only_empty_name_yields_text_content() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_x".into(),
                name: "".into(),
                input: json!({}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, &anthropic_compat());
        let content = result.iter().find(|m| m["role"] == "assistant").unwrap()["content"]
            .as_array()
            .unwrap();
        assert!(content.iter().any(|b| {
            b["type"] == "text"
                && b["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("[tool call skipped:")
                && b["text"].as_str().unwrap_or("").contains("arguments={}")
        }));
        assert!(
            !content
                .iter()
                .any(|b| b["type"] == "tool_use" && b["name"] == "")
        );
    }

    #[test]
    fn test_anthropic_downgrade_text_not_stripped() {
        let mut compat = anthropic_compat();
        compat.messages.strip_patterns = Some(vec![
            "REMOVE_ME".into(),
            "[tool call skipped:".into(),
            "arguments={}".into(),
        ]);
        let messages = vec![Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "ordinary REMOVE_ME text".into(),
                },
                ContentBlock::ToolUse {
                    id: "call_x".into(),
                    name: "".into(),
                    input: json!({}),
                    extra: None,
                },
            ],
        )];

        let result = build_messages(&messages, &compat);
        let content = result.iter().find(|m| m["role"] == "assistant").unwrap()["content"]
            .as_array()
            .unwrap();
        assert!(content.iter().any(|b| {
            b["type"] == "text" && b["text"].as_str().unwrap_or("") == "ordinary  text"
        }));
        assert!(content.iter().any(|b| {
            b["type"] == "text"
                && b["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("[tool call skipped:")
                && b["text"].as_str().unwrap_or("").contains("arguments={}")
        }));
    }

    #[test]
    fn test_anthropic_sanitize_disabled_keeps_empty_name() {
        let mut compat = anthropic_compat();
        compat.tools.sanitize_malformed_tool_calls = Some(false);
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_x".into(),
                name: "".into(),
                input: json!({}),
                extra: None,
            }],
        )];

        let result = build_messages(&messages, &compat);
        let any_empty = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_use" && b["name"] == "");
        assert!(any_empty);
    }

    #[test]
    fn test_anthropic_reverse_orphan_tool_result_dropped() {
        let messages = vec![Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "missing".into(),
                content: "orphan".into(),
                is_error: true,
            }],
        )];

        let result = build_messages(&messages, &anthropic_compat());
        let any_tool_result = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "missing");
        assert!(!any_tool_result);
        assert!(result.iter().any(|m| {
            m["role"] == "user"
                && m["content"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|b| {
                        b["type"] == "text"
                            && b["text"].as_str().unwrap_or("")
                                == "[tool call skipped: malformed (orphan tool result).]"
                    })
                })
        }));
    }

    #[test]
    fn test_anthropic_reverse_orphan_matched_result_not_dropped() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_x".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_x".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];

        let result = build_messages(&messages, &anthropic_compat());
        let any_tool_result = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "call_x");
        assert!(any_tool_result);
    }

    #[test]
    fn test_anthropic_empty_id_toolcall_downgraded_when_auto_id_disabled() {
        let mut compat = anthropic_compat();
        compat.tools.auto_tool_id = Some(false);
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "".into(),
                name: "Bash".into(),
                input: json!({"command":"ls"}),
                extra: None,
            }],
        )];

        let result = build_messages(&messages, &compat);
        let blocks: Vec<_> = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .collect();
        assert!(!blocks.iter().any(|b| b["type"] == "tool_use"));
        assert!(!blocks.iter().any(|b| b["type"] == "tool_result"));
        assert!(blocks.iter().any(|b| {
            b["type"] == "text"
                && b["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("empty tool call id")
                && b["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("arguments={\"command\":\"ls\"}")
        }));
    }

    #[test]
    fn test_anthropic_empty_id_toolcall_generates_id_when_auto_id_enabled() {
        let mut compat = anthropic_compat();
        compat.tools.auto_tool_id = Some(true);
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "".into(),
                    content: "orphan".into(),
                    is_error: true,
                }],
            ),
        ];

        let result = build_messages(&messages, &compat);
        let blocks: Vec<_> = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .collect();
        let tool_use = blocks.iter().find(|b| b["type"] == "tool_use").unwrap();
        assert_eq!(tool_use["name"], "Bash");
        assert!(tool_use["id"].as_str().unwrap().starts_with("toolu_"));
        assert_ne!(tool_use["id"], "");
    }

    #[test]
    fn test_anthropic_empty_id_rewrites_paired_result_when_auto_id_enabled() {
        let mut compat = anthropic_compat();
        compat.tools.auto_tool_id = Some(true);
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];

        let result = build_messages(&messages, &compat);
        let blocks: Vec<_> = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .collect();
        let tool_use = blocks.iter().find(|b| b["type"] == "tool_use").unwrap();
        let generated_id = tool_use["id"].as_str().unwrap();
        assert!(generated_id.starts_with("toolu_"));
        let tool_result = blocks.iter().find(|b| b["type"] == "tool_result").unwrap();
        assert_eq!(tool_result["tool_use_id"], generated_id);
        assert_eq!(tool_result["content"], "ok");
    }

    #[test]
    fn test_anthropic_result_before_matching_call_is_dropped() {
        let messages = vec![
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "late".into(),
                    content: "too early".into(),
                    is_error: true,
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "late".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    extra: None,
                }],
            ),
        ];

        let result = build_messages(&messages, &anthropic_compat());
        let blocks: Vec<_> = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .collect();
        assert!(
            !blocks
                .iter()
                .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "late")
        );
    }

    #[test]
    fn test_anthropic_dropped_empty_id_does_not_consume_later_generated_empty_id_result() {
        let mut compat = anthropic_compat();
        compat.tools.auto_tool_id = Some(true);
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "".into(),
                    name: "".into(),
                    input: json!({"bad":true}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "".into(),
                    content: "bad result".into(),
                    is_error: true,
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];

        let result = build_messages(&messages, &compat);
        let blocks: Vec<_> = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .collect();
        let generated_id = blocks
            .iter()
            .find(|b| b["type"] == "tool_use" && b["name"] == "Bash")
            .and_then(|b| b["id"].as_str())
            .unwrap();
        let tool_results: Vec<_> = blocks
            .iter()
            .filter(|b| b["type"] == "tool_result")
            .collect();
        assert_eq!(tool_results.len(), 1);
        assert_eq!(tool_results[0]["tool_use_id"], generated_id);
        assert_eq!(tool_results[0]["content"], "ok");
    }

    #[test]
    fn test_anthropic_dropped_tool_result_yields_placeholder_content() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_x".into(),
                    name: "".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_x".into(),
                    content: "Unknown tool: ".into(),
                    is_error: true,
                }],
            ),
        ];

        let result = build_messages(&messages, &anthropic_compat());
        let user_placeholder = result.iter().any(|m| {
            m["role"] == "user"
                && m["content"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|b| {
                        b["type"] == "text"
                            && b["text"].as_str().unwrap_or("")
                                == "[tool call skipped: malformed (empty function name).]"
                    })
                })
        });
        assert!(user_placeholder);
        let any_tool_result = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "call_x");
        assert!(!any_tool_result);
    }

    // H1-2
    #[test]
    fn test_anthropic_normal_toolcall_unaffected() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_x".into(),
                name: "Bash".into(),
                input: json!({"command":"ls"}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, &anthropic_compat());
        let any_bash = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_use" && b["name"] == "Bash");
        assert!(any_bash);
    }

    // --- build_tools tests ---

    #[test]
    fn test_build_tools_single() {
        // arrange
        let schema = json!({
            "type": "object",
            "properties": {
                "cmd": { "type": "string" }
            },
            "required": ["cmd"]
        });
        let tools = vec![ToolDef {
            name: "bash".to_string(),
            description: "Run a shell command".to_string(),
            input_schema: schema.clone(),
            deferred: false,
        }];
        // act
        let result = build_tools(&tools);
        // assert
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "bash");
        assert_eq!(result[0]["description"], "Run a shell command");
        assert_eq!(result[0]["input_schema"], schema);
    }

    #[test]
    fn test_build_tools_empty() {
        // arrange
        let tools: Vec<ToolDef> = vec![];
        // act
        let result = build_tools(&tools);
        // assert
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_tools_deferred_has_empty_schema() {
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
        let result = build_tools(&tools);

        // Core tool has full input_schema
        assert!(
            result[0]["input_schema"]["properties"]
                .get("path")
                .is_some()
        );

        // Deferred tool has empty input_schema and modified description
        assert!(
            result[1]["input_schema"]["properties"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        let desc = result[1]["description"].as_str().unwrap();
        assert!(desc.contains("ToolSearch"));
    }

    // --- parse_sse_data tests ---

    #[test]
    fn test_parse_anthropic_event_text_delta() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{"delta":{"type":"text_delta","text":"Hello"}}"#;
        // act
        let events = parse_sse_data("content_block_delta", data, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::TextDelta(t) => assert_eq!(t, "Hello"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_tool_use() {
        // arrange
        let mut state = StreamState::new();
        // step 1: content_block_start with tool_use type
        let start_events = parse_sse_data(
            "content_block_start",
            r#"{"content_block":{"type":"tool_use","id":"id1","name":"bash"}}"#,
            &mut state,
        );
        assert!(start_events.is_empty());
        // step 2: content_block_delta with input_json_delta
        let delta_events = parse_sse_data(
            "content_block_delta",
            r#"{"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ls\"}"}}"#,
            &mut state,
        );
        assert!(delta_events.is_empty());
        // step 3: content_block_stop emits the ToolUse event
        let events = parse_sse_data("content_block_stop", r#"{}"#, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "id1");
                assert_eq!(name, "bash");
                assert_eq!(input["cmd"], "ls");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    // F1-10
    #[test]
    fn test_empty_name_tool_use_still_emitted_to_history() {
        let mut state = StreamState::new();

        let start_events = parse_sse_data(
            "content_block_start",
            r#"{"content_block":{"type":"tool_use","id":"call_x","name":""}}"#,
            &mut state,
        );
        assert!(start_events.is_empty());

        let events = parse_sse_data("content_block_stop", r#"{}"#, &mut state);
        assert_eq!(events.len(), 1);

        match &events[0] {
            LlmEvent::ToolUse { id, name, .. } => {
                assert_eq!(id, "call_x");
                assert_eq!(name, "");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_stop() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#;
        // act
        let events = parse_sse_data("message_delta", data, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.output_tokens, 42);
            }
            _ => panic!("expected Done"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_thinking() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{"delta":{"type":"thinking_delta","thinking":"reasoning step"}}"#;
        // act
        let events = parse_sse_data("content_block_delta", data, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ThinkingDelta(t) => assert_eq!(t, "reasoning step"),
            _ => panic!("expected ThinkingDelta"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_thinking_signature() {
        let mut state = StreamState::new();
        let data = r#"{"delta":{"type":"signature_delta","signature":"sig-123"}}"#;

        let events = parse_sse_data("content_block_delta", data, &mut state);

        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ThinkingSignature(signature) => assert_eq!(signature, "sig-123"),
            _ => panic!("expected ThinkingSignature"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_unknown_type() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{}"#;
        // act
        let events = parse_sse_data("unknown_event", data, &mut state);
        // assert
        assert!(events.is_empty());
    }
}
