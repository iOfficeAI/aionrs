use std::collections::{HashMap, HashSet, VecDeque};

use aion_config::compat::ProviderCompat;
use aion_types::message::{ContentBlock, Message, Role};
use serde_json::{Value, json};

use crate::tool_call_sanitize::{DroppedToolCallReason, format_dropped_tool_call};

pub(crate) fn build_messages(messages: &[Message], system: &str, compat: &ProviderCompat) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();
    let sanitize = compat.sanitize_malformed_tool_calls();
    let auto_tool_id = compat.auto_tool_id();
    let clean_orphan_tool_results = compat.clean_orphan_tool_results();
    // tool_call ids dropped as malformed; their paired tool results must be
    // skipped later to avoid orphan "tool" messages.
    let mut dropped_ids: HashMap<String, VecDeque<DroppedToolCallReason>> = HashMap::new();
    let mut available_tool_call_ids: HashSet<String> = HashSet::new();
    let mut generated_tool_call_ids: HashMap<String, VecDeque<String>> = HashMap::new();

    // System message first
    if !system.is_empty() {
        result.push(json!({
            "role": "system",
            "content": system
        }));
    }

    for msg in messages {
        match msg.role {
            Role::User => {
                // Check if this contains tool results
                let has_tool_results = msg.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }));

                if has_tool_results {
                    // Each tool result becomes a separate "tool" role message
                    for block in &msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id, content, ..
                        } = block
                        {
                            if let Some(reasons) = dropped_ids.get_mut(tool_use_id)
                                && reasons.pop_front().is_some()
                            {
                                continue;
                            }
                            let projected_tool_use_id = generated_tool_call_ids
                                .get_mut(tool_use_id)
                                .and_then(VecDeque::pop_front)
                                .unwrap_or_else(|| tool_use_id.clone());
                            if clean_orphan_tool_results && !available_tool_call_ids.contains(&projected_tool_use_id) {
                                tracing::warn!(
                                    target: "aion_providers",
                                    tool_call_id = %tool_use_id,
                                    reason = "orphan_result",
                                    "dropped orphan tool_result in outgoing request"
                                );
                                continue;
                            }
                            result.push(json!({
                                "role": "tool",
                                "tool_call_id": projected_tool_use_id,
                                "content": content
                            }));
                        }
                    }
                } else {
                    let text: String = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let text = strip_patterns_from_text(&text, compat);
                    result.push(json!({
                        "role": "user",
                        "content": text
                    }));
                }
            }
            Role::Assistant => {
                let mut msg_json = json!({ "role": "assistant" });

                // Preserve only reasoning content that belongs to this
                // assistant message. Provider-specific blanket replay
                // policy must remain an explicit compat decision.
                let thinking: String = msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Thinking { thinking, .. } = b {
                            Some(thinking.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");

                if !thinking.is_empty() {
                    msg_json["reasoning_content"] = json!(thinking);
                }

                let text: String = msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Text { text } = b {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let text = strip_patterns_from_text(&text, compat);

                let mut tool_calls: Vec<Value> = Vec::new();
                let mut dropped_lines: Vec<String> = Vec::new();
                for b in &msg.content {
                    if let ContentBlock::ToolUse { id, name, input, extra } = b {
                        if sanitize && name.is_empty() {
                            dropped_ids
                                .entry(id.clone())
                                .or_default()
                                .push_back(DroppedToolCallReason::EmptyName);
                            dropped_lines.push(format_dropped_tool_call(DroppedToolCallReason::EmptyName, input));
                            tracing::warn!(
                                target: "aion_providers",
                                tool_call_id = %id,
                                reason = DroppedToolCallReason::EmptyName.log_reason(),
                                "downgraded malformed tool_call to text in outgoing request"
                            );
                            continue;
                        }

                        if sanitize && id.is_empty() && !auto_tool_id {
                            dropped_ids
                                .entry(id.clone())
                                .or_default()
                                .push_back(DroppedToolCallReason::EmptyId);
                            dropped_lines.push(format_dropped_tool_call(DroppedToolCallReason::EmptyId, input));
                            tracing::warn!(
                                target: "aion_providers",
                                tool_call_id = %id,
                                reason = DroppedToolCallReason::EmptyId.log_reason(),
                                "downgraded malformed tool_call to text in outgoing request"
                            );
                            continue;
                        }

                        let tool_id = if id.is_empty() && auto_tool_id {
                            generate_call_id()
                        } else {
                            id.clone()
                        };
                        if id.is_empty() && auto_tool_id {
                            generated_tool_call_ids
                                .entry(id.clone())
                                .or_default()
                                .push_back(tool_id.clone());
                        }
                        available_tool_call_ids.insert(tool_id.clone());
                        let mut tc_json = json!({
                            "id": tool_id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": serde_json::to_string(input).unwrap_or_default()
                            }
                        });
                        if let Some(extra_val) = extra {
                            tc_json["extra_content"] = extra_val.clone();
                        }
                        tool_calls.push(tc_json);
                    }
                }

                // Compose content: original text + downgrade lines.
                let mut content_parts: Vec<String> = Vec::new();
                if !text.is_empty() {
                    content_parts.push(text.clone());
                }
                content_parts.extend(dropped_lines);
                let combined = content_parts.join("\n\n");

                if !combined.is_empty() {
                    msg_json["content"] = json!(combined);
                } else if tool_calls.is_empty() {
                    msg_json["content"] = json!("");
                }

                if !tool_calls.is_empty() {
                    msg_json["tool_calls"] = json!(tool_calls);
                }

                result.push(msg_json);
            }
            Role::System => {
                // Already handled above
            }
            Role::Tool => {
                for block in &msg.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id, content, ..
                    } = block
                    {
                        if let Some(reasons) = dropped_ids.get_mut(tool_use_id)
                            && reasons.pop_front().is_some()
                        {
                            continue;
                        }
                        let projected_tool_use_id = generated_tool_call_ids
                            .get_mut(tool_use_id)
                            .and_then(VecDeque::pop_front)
                            .unwrap_or_else(|| tool_use_id.clone());
                        if clean_orphan_tool_results && !available_tool_call_ids.contains(&projected_tool_use_id) {
                            tracing::warn!(
                                target: "aion_providers",
                                tool_call_id = %tool_use_id,
                                reason = "orphan_result",
                                "dropped orphan tool_result in outgoing request"
                            );
                            continue;
                        }
                        result.push(json!({
                            "role": "tool",
                            "tool_call_id": projected_tool_use_id,
                            "content": content
                        }));
                    }
                }
            }
        }
    }

    // Dedup tool results: keep last occurrence of each tool_call_id
    if compat.dedup_tool_results() {
        dedup_tool_results(&mut result);
    }

    // Clean orphan tool calls: remove tool_call entries with no matching tool result
    if compat.clean_orphan_tool_calls() {
        clean_orphaned_tool_calls(&mut result, !sanitize);
    }

    // Merge consecutive assistant messages
    if compat.merge_assistant_messages() {
        merge_consecutive_assistant(&mut result);
    }

    result
}

/// Generate a unique tool call ID in OpenAI `call_xxx` format.
pub(crate) fn generate_call_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let rand: u64 = (ts as u64).wrapping_mul(6364136223846793005);
    format!("call_{:016x}", rand)
}

/// Strip configured patterns from text content.
fn strip_patterns_from_text(text: &str, compat: &ProviderCompat) -> String {
    match &compat.messages.strip_patterns {
        Some(patterns) if !patterns.is_empty() => {
            let mut result = text.to_string();
            for pattern in patterns {
                result = result.replace(pattern, "");
            }
            result
        }
        _ => text.to_string(),
    }
}

/// Deduplicate tool results: keep last occurrence of each tool_call_id.
fn dedup_tool_results(messages: &mut Vec<Value>) {
    // Find the last index of each tool_call_id
    let mut last_index: HashMap<String, usize> = HashMap::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg["role"].as_str() == Some("tool")
            && let Some(id) = msg["tool_call_id"].as_str()
        {
            last_index.insert(id.to_string(), i);
        }
    }

    // Keep only the last occurrence
    let mut seen: HashMap<String, bool> = HashMap::new();
    let mut to_remove = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg["role"].as_str() == Some("tool")
            && let Some(id) = msg["tool_call_id"].as_str()
            && let Some(&last_i) = last_index.get(id)
        {
            if i != last_i && !seen.contains_key(id) {
                to_remove.push(i);
            }
            if i == last_i {
                seen.insert(id.to_string(), true);
            }
        }
    }

    // Remove in reverse order to preserve indices
    for i in to_remove.into_iter().rev() {
        messages.remove(i);
    }
}

/// Remove tool_call entries from assistant messages that have no corresponding tool result.
fn clean_orphaned_tool_calls(messages: &mut [Value], retain_empty_name_tool_calls: bool) {
    let answered_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m["role"].as_str() == Some("tool"))
        .filter_map(|m| m["tool_call_id"].as_str().map(String::from))
        .collect();

    for msg in messages.iter_mut() {
        if msg["role"].as_str() == Some("assistant")
            && let Some(tcs) = msg.get_mut("tool_calls").and_then(Value::as_array_mut)
        {
            tcs.retain(|tc| {
                if retain_empty_name_tool_calls && tc["function"]["name"].as_str() == Some("") {
                    return true;
                }
                tc["id"].as_str().map(|id| answered_ids.contains(id)).unwrap_or(true)
            });
            if tcs.is_empty() {
                msg.as_object_mut().unwrap().remove("tool_calls");
                if msg.get("content").is_none() {
                    msg["content"] = json!("");
                }
            }
        }
    }
}

/// Merge consecutive assistant messages into one.
fn merge_consecutive_assistant(messages: &mut Vec<Value>) {
    let mut i = 0;
    while i + 1 < messages.len() {
        if messages[i]["role"].as_str() == Some("assistant") && messages[i + 1]["role"].as_str() == Some("assistant") {
            let next = messages.remove(i + 1);

            // Merge text content
            let curr_text = messages[i]["content"].as_str().unwrap_or("").to_string();
            let next_text = next["content"].as_str().unwrap_or("").to_string();
            let merged_text = match (curr_text.is_empty(), next_text.is_empty()) {
                (true, true) => String::new(),
                (true, false) => next_text,
                (false, true) => curr_text,
                (false, false) => format!("{}{}", curr_text, next_text),
            };

            if !merged_text.is_empty() {
                messages[i]["content"] = json!(merged_text);
            }

            // Merge reasoning_content
            let curr_rc = messages[i]["reasoning_content"].as_str().unwrap_or("").to_string();
            let next_rc = next["reasoning_content"].as_str().unwrap_or("").to_string();
            let merged_rc = match (curr_rc.is_empty(), next_rc.is_empty()) {
                (true, true) => String::new(),
                (true, false) => next_rc,
                (false, true) => curr_rc,
                (false, false) => format!("{}{}", curr_rc, next_rc),
            };

            if !merged_rc.is_empty() {
                messages[i]["reasoning_content"] = json!(merged_rc);
            } else if let Some(obj) = messages[i].as_object_mut() {
                obj.remove("reasoning_content");
            }

            // Merge tool_calls
            if let Some(next_tcs) = next["tool_calls"].as_array() {
                let curr_tcs = messages[i]
                    .as_object_mut()
                    .unwrap()
                    .entry("tool_calls")
                    .or_insert_with(|| json!([]));
                if let Some(arr) = curr_tcs.as_array_mut() {
                    arr.extend(next_tcs.iter().cloned());
                }
            }

            // Don't increment i - check the merged result against the next message
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aion_types::message::{ContentBlock, Message, Role};

    fn no_compat() -> ProviderCompat {
        ProviderCompat::default()
    }

    fn openai_compat() -> ProviderCompat {
        ProviderCompat::openai_defaults()
    }

    #[test]
    fn test_merge_assistant_messages_enabled() {
        let messages = vec![
            Message::new(Role::Assistant, vec![ContentBlock::Text { text: "hello".into() }]),
            Message::new(Role::Assistant, vec![ContentBlock::Text { text: " world".into() }]),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 1);
        assert_eq!(assistant_msgs[0]["content"], "hello world");
    }

    #[test]
    fn test_merge_assistant_messages_disabled() {
        let messages = vec![
            Message::new(Role::Assistant, vec![ContentBlock::Text { text: "hello".into() }]),
            Message::new(Role::Assistant, vec![ContentBlock::Text { text: " world".into() }]),
        ];
        let result = build_messages(&messages, "", &no_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 2);
    }

    #[test]
    fn test_reasoning_content_projects_only_message_thinking() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "q1".into() }]),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Thinking {
                        thinking: "private chain".into(),
                        signature: None,
                    },
                    ContentBlock::Text {
                        text: "first answer".into(),
                    },
                ],
            ),
            Message::new(Role::User, vec![ContentBlock::Text { text: "q2".into() }]),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "second answer".into(),
                }],
            ),
        ];

        let result = build_messages(&messages, "", &openai_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();

        assert_eq!(assistant_msgs.len(), 2);
        assert_eq!(assistant_msgs[0]["reasoning_content"], "private chain");
        assert!(
            assistant_msgs[1].get("reasoning_content").is_none(),
            "assistant messages without Thinking blocks must not receive empty reasoning_content"
        );
    }

    #[test]
    fn test_reasoning_content_merge_drops_empty_replay_values() {
        let mut messages = vec![
            json!({
                "role": "assistant",
                "content": "first",
                "reasoning_content": ""
            }),
            json!({
                "role": "assistant",
                "content": " second"
            }),
        ];

        merge_consecutive_assistant(&mut messages);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["content"], "first second");
        assert!(
            messages[0].get("reasoning_content").is_none(),
            "merged assistant message should not retain an empty reasoning_content field"
        );
    }

    #[test]
    fn test_forward_and_reverse_orphan_cleanup_do_not_conflict() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "matched".into(),
                        name: "Bash".into(),
                        input: json!({"command":"pwd"}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "forward_orphan".into(),
                        name: "Read".into(),
                        input: json!({"file_path":"x"}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "matched".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "reverse_orphan".into(),
                    content: "bad".into(),
                    is_error: true,
                }],
            ),
        ];

        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["id"], "matched");
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["tool_call_id"], "matched");
    }

    #[test]
    fn test_clean_orphan_tool_calls_enabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "tc1".into(),
                        name: "bash".into(),
                        input: json!({}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tc2".into(),
                        name: "read".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
            // tc2 has no result -> orphan
        ];
        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["id"], "tc1");
    }

    #[test]
    fn test_clean_orphan_tool_calls_disabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "tc1".into(),
                        name: "bash".into(),
                        input: json!({}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tc2".into(),
                        name: "read".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &no_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 2);
    }

    #[test]
    fn test_reverse_orphan_tool_result_dropped() {
        let messages = vec![Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "missing".into(),
                content: "orphan".into(),
                is_error: true,
            }],
        )];
        let result = build_messages(&messages, "", &openai_compat());
        assert!(result.iter().all(|m| m["role"] != "tool"));
    }

    #[test]
    fn test_reverse_orphan_tool_result_kept_when_disabled() {
        let mut compat = openai_compat();
        compat.messages.clean_orphan_tool_results = Some(false);
        let messages = vec![Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "missing".into(),
                content: "orphan".into(),
                is_error: true,
            }],
        )];
        let result = build_messages(&messages, "", &compat);
        assert!(
            result
                .iter()
                .any(|m| { m["role"] == "tool" && m["tool_call_id"] == "missing" && m["content"] == "orphan" })
        );
    }

    #[test]
    fn test_matched_tool_result_not_dropped_by_reverse_cleanup() {
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
        let result = build_messages(&messages, "", &openai_compat());
        assert!(
            result
                .iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == "call_x")
        );
    }

    #[test]
    fn test_empty_id_toolcall_downgraded_when_auto_id_disabled() {
        let mut compat = openai_compat();
        compat.tools.auto_tool_id = Some(false);
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
        let result = build_messages(&messages, "", &compat);
        assert!(result.iter().all(|m| m["role"] != "tool"));
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert!(assistant.get("tool_calls").is_none());
        let content = assistant["content"].as_str().unwrap();
        assert!(content.contains("[tool call skipped:"));
        assert!(content.contains("empty tool call id"));
        assert!(content.contains("arguments={\"command\":\"ls\"}"));
    }

    #[test]
    fn test_empty_id_toolcall_generates_id_when_auto_id_enabled() {
        let mut compat = openai_compat();
        compat.tools.auto_tool_id = Some(true);
        compat.tools.clean_orphan_tool_calls = Some(false);
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "".into(),
                name: "Bash".into(),
                input: json!({"command":"ls"}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, "", &compat);
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tc = &assistant["tool_calls"][0];
        assert_eq!(tc["function"]["name"], "Bash");
        assert!(tc["id"].as_str().unwrap().starts_with("call_"));
        assert_ne!(tc["id"], "");
    }

    #[test]
    fn test_empty_id_toolcall_rewrites_paired_result_when_auto_id_enabled() {
        let mut compat = openai_compat();
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
        let result = build_messages(&messages, "", &compat);
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let generated_id = assistant["tool_calls"][0]["id"].as_str().unwrap();
        assert!(generated_id.starts_with("call_"));
        let tool = result.iter().find(|m| m["role"] == "tool").unwrap();
        assert_eq!(tool["tool_call_id"], generated_id);
        assert_eq!(tool["content"], "ok");
    }

    #[test]
    fn test_result_before_matching_call_is_dropped() {
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
        let result = build_messages(&messages, "", &openai_compat());
        assert!(result.iter().all(|m| m["role"] != "tool"));
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert!(assistant.get("tool_calls").is_none());
        assert_eq!(assistant["content"], "");
    }

    #[test]
    fn test_dropped_empty_id_does_not_consume_later_generated_empty_id_result() {
        let mut compat = openai_compat();
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
        let result = build_messages(&messages, "", &compat);
        let assistant_with_call = result
            .iter()
            .find(|m| m["tool_calls"].as_array().is_some_and(|calls| !calls.is_empty()))
            .unwrap();
        let generated_id = assistant_with_call["tool_calls"][0]["id"].as_str().unwrap();
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["tool_call_id"], generated_id);
        assert_eq!(tool_msgs[0]["content"], "ok");
    }

    #[test]
    fn test_empty_name_toolcall_downgraded_and_paired_result_dropped() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Text { text: "writing".into() },
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
        let result = build_messages(&messages, "", &openai_compat());
        assert!(
            result.iter().all(|m| m["role"] != "tool"),
            "paired tool result must be dropped"
        );
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let has_empty = assistant
            .get("tool_calls")
            .and_then(|t| t.as_array())
            .map(|a| a.iter().any(|tc| tc["function"]["name"] == ""))
            .unwrap_or(false);
        assert!(!has_empty, "no empty-name tool_call in projection");
        assert!(assistant["content"].as_str().unwrap().contains("[tool call skipped:"));
        assert!(assistant["content"].as_str().unwrap().contains("writing"));
    }

    #[test]
    fn test_mixed_valid_and_empty_name() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "ok".into(),
                        name: "Bash".into(),
                        input: json!({"command":"ls"}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "bad".into(),
                        name: "".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "ok".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "bad".into(),
                    content: "Unknown tool: ".into(),
                    is_error: true,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["function"]["name"], "Bash");
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["tool_call_id"], "ok");
    }

    #[test]
    fn test_only_empty_name_yields_placeholder_content() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_x".into(),
                name: "".into(),
                input: json!({}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert!(assistant.get("tool_calls").is_none());
        let content = assistant["content"].as_str().unwrap();
        assert!(content.contains("[tool call skipped:"));
        assert!(content.contains("arguments={}"));
    }

    #[test]
    fn test_thinking_only_assistant_keeps_empty_content() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Thinking {
                thinking: "internal reasoning".into(),
                signature: None,
            }],
        )];
        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert_eq!(assistant["content"], "");
        assert!(!assistant["content"].as_str().unwrap().contains("malformed"));
    }

    #[test]
    fn test_empty_name_toolcall_with_user_tool_result_dropped() {
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
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_x".into(),
                    content: "Unknown tool: ".into(),
                    is_error: true,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        assert!(result.iter().all(|m| m["role"] != "tool"));
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert!(assistant["content"].as_str().unwrap().contains("[tool call skipped:"));
    }

    #[test]
    fn test_two_empty_name_calls_produce_two_lines() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![
                ContentBlock::ToolUse {
                    id: "a".into(),
                    name: "".into(),
                    input: json!({"x":1}),
                    extra: None,
                },
                ContentBlock::ToolUse {
                    id: "b".into(),
                    name: "".into(),
                    input: json!({"y":2}),
                    extra: None,
                },
            ],
        )];
        let result = build_messages(&messages, "", &openai_compat());
        let content = result.iter().find(|m| m["role"] == "assistant").unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(content.matches("[tool call skipped:").count(), 2);
        assert!(content.contains("{\"x\":1}") && content.contains("{\"y\":2}"));
    }

    #[test]
    fn test_sanitize_disabled_keeps_empty_name() {
        let mut compat = openai_compat();
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
        let result = build_messages(&messages, "", &compat);
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "");
    }

    #[test]
    fn test_normal_toolcall_unaffected() {
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
        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "Bash");
        assert!(
            result
                .iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == "call_x")
        );
    }

    #[test]
    fn test_dedup_tool_results_enabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tc1".into(),
                    name: "bash".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "first".into(),
                    is_error: false,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "second".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["content"], "second");
    }
}
