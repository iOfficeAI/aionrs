use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::types::llm::{LlmEvent, LlmRequest};
use crate::types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use crate::types::tool::ToolDef;

use super::compat::ProviderCompat;
use super::{LlmProvider, ProviderError};

pub struct OpenAIProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
}

impl OpenAIProvider {
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            compat,
        }
    }

    fn build_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let bearer = format!("Bearer {}", self.api_key);
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&bearer).unwrap());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers
    }

    fn build_messages(messages: &[Message], system: &str, compat: &ProviderCompat) -> Vec<Value> {
        let mut result: Vec<Value> = Vec::new();

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
                    let has_tool_results = msg
                        .content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

                    if has_tool_results {
                        // Each tool result becomes a separate "tool" role message
                        for block in &msg.content {
                            if let ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } = block
                            {
                                result.push(json!({
                                    "role": "tool",
                                    "tool_call_id": tool_use_id,
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

                    if !text.is_empty() {
                        msg_json["content"] = json!(text);
                    } else {
                        msg_json["content"] = Value::Null;
                    }

                    let tool_calls: Vec<Value> = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::ToolUse { id, name, input } = b {
                                Some(json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": serde_json::to_string(input).unwrap_or_default()
                                    }
                                }))
                            } else {
                                None
                            }
                        })
                        .collect();

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
                            tool_use_id,
                            content,
                            ..
                        } = block
                        {
                            result.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
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
            clean_orphaned_tool_calls(&mut result);
        }

        // Merge consecutive assistant messages
        if compat.merge_assistant_messages() {
            merge_consecutive_assistant(&mut result);
        }

        result
    }

    fn build_tools(tools: &[ToolDef]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema
                    }
                })
            })
            .collect()
    }

    fn build_request_body(&self, request: &LlmRequest) -> Value {
        let max_tokens_field = self
            .compat
            .max_tokens_field
            .as_deref()
            .unwrap_or("max_tokens");

        let mut body = json!({
            "model": request.model,
            "messages": Self::build_messages(&request.messages, &request.system, &self.compat),
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        body[max_tokens_field] = json!(request.max_tokens);

        if !request.tools.is_empty() {
            body["tools"] = json!(Self::build_tools(&request.tools));
        }

        if let Some(effort) = &request.reasoning_effort {
            body["reasoning_effort"] = json!(effort);
        }

        body
    }
}

/// Strip configured patterns from text content
fn strip_patterns_from_text(text: &str, compat: &ProviderCompat) -> String {
    match &compat.strip_patterns {
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

/// Deduplicate tool results: keep last occurrence of each tool_call_id
fn dedup_tool_results(messages: &mut Vec<Value>) {
    use std::collections::HashMap;

    // Find the last index of each tool_call_id
    let mut last_index: HashMap<String, usize> = HashMap::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg["role"].as_str() == Some("tool") {
            if let Some(id) = msg["tool_call_id"].as_str() {
                last_index.insert(id.to_string(), i);
            }
        }
    }

    // Keep only the last occurrence
    let mut seen: HashMap<String, bool> = HashMap::new();
    let mut to_remove = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg["role"].as_str() == Some("tool") {
            if let Some(id) = msg["tool_call_id"].as_str() {
                if let Some(&last_i) = last_index.get(id) {
                    if i != last_i && !seen.contains_key(id) {
                        to_remove.push(i);
                    }
                    if i == last_i {
                        seen.insert(id.to_string(), true);
                    }
                }
            }
        }
    }

    // Remove in reverse order to preserve indices
    for i in to_remove.into_iter().rev() {
        messages.remove(i);
    }
}

/// Remove tool_call entries from assistant messages that have no corresponding tool result
fn clean_orphaned_tool_calls(messages: &mut Vec<Value>) {
    use std::collections::HashSet;

    let answered_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m["role"].as_str() == Some("tool"))
        .filter_map(|m| m["tool_call_id"].as_str().map(String::from))
        .collect();

    for msg in messages.iter_mut() {
        if msg["role"].as_str() == Some("assistant") {
            if let Some(tcs) = msg["tool_calls"].as_array_mut() {
                tcs.retain(|tc| {
                    tc["id"]
                        .as_str()
                        .map(|id| answered_ids.contains(id))
                        .unwrap_or(true)
                });
                if tcs.is_empty() {
                    msg.as_object_mut().unwrap().remove("tool_calls");
                }
            }
        }
    }
}

/// Merge consecutive assistant messages into one
fn merge_consecutive_assistant(messages: &mut Vec<Value>) {
    let mut i = 0;
    while i + 1 < messages.len() {
        if messages[i]["role"].as_str() == Some("assistant")
            && messages[i + 1]["role"].as_str() == Some("assistant")
        {
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

            // Don't increment i — check the merged result against the next message
        } else {
            i += 1;
        }
    }
}

/// State for accumulating tool call deltas by index
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

struct StreamState {
    tool_calls: Vec<ToolCallAccumulator>,
    input_tokens: u64,
    output_tokens: u64,
}

impl StreamState {
    fn new() -> Self {
        Self {
            tool_calls: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    fn get_or_create_tool(&mut self, index: usize) -> &mut ToolCallAccumulator {
        while self.tool_calls.len() <= index {
            self.tool_calls.push(ToolCallAccumulator {
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
            });
        }
        &mut self.tool_calls[index]
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = format!("{}{}", self.base_url, self.compat.api_path());
        let body = self.build_request_body(request);

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers())
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimited {
                    retry_after_ms: 5000,
                });
            }
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        let (tx, rx) = mpsc::channel(64);

        tokio::spawn(async move {
            if let Err(e) = process_sse_stream(response, &tx).await {
                let _ = tx.send(LlmEvent::Error(e.to_string())).await;
            }
        });

        Ok(rx)
    }
}

async fn process_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
) -> Result<(), ProviderError> {
    use futures::StreamExt;

    let mut state = StreamState::new();
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ProviderError::Connection(e.to_string()))?;
        let text = String::from_utf8_lossy(&chunk);
        buffer.push_str(&text);

        // Process complete lines
        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    return Ok(());
                }

                let events = parse_sse_chunk(data, &mut state);
                for event in events {
                    if tx.send(event).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    Ok(())
}

fn parse_sse_chunk(data: &str, state: &mut StreamState) -> Vec<LlmEvent> {
    let mut events = Vec::new();

    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return events,
    };

    // Extract usage if present
    if let Some(usage) = json.get("usage") {
        state.input_tokens = usage["prompt_tokens"].as_u64().unwrap_or(state.input_tokens);
        state.output_tokens = usage["completion_tokens"]
            .as_u64()
            .unwrap_or(state.output_tokens);
    }

    let Some(choice) = json["choices"].as_array().and_then(|c| c.first()) else {
        return events;
    };

    let delta = &choice["delta"];

    // Reasoning content (OpenAI reasoning models)
    if let Some(reasoning) = delta["reasoning_content"].as_str() {
        if !reasoning.is_empty() {
            events.push(LlmEvent::ThinkingDelta(reasoning.to_string()));
        }
    }

    // Text content
    if let Some(content) = delta["content"].as_str() {
        if !content.is_empty() {
            events.push(LlmEvent::TextDelta(content.to_string()));
        }
    }

    // Tool calls
    if let Some(tool_calls) = delta["tool_calls"].as_array() {
        for tc in tool_calls {
            let index = tc["index"].as_u64().unwrap_or(0) as usize;
            let acc = state.get_or_create_tool(index);

            if let Some(id) = tc["id"].as_str() {
                acc.id = id.to_string();
            }
            if let Some(name) = tc["function"]["name"].as_str() {
                acc.name = name.to_string();
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                acc.arguments.push_str(args);
            }
        }
    }

    // Check finish_reason
    if let Some(finish_reason) = choice["finish_reason"].as_str() {
        match finish_reason {
            "tool_calls" => {
                // Emit accumulated tool calls
                for tc in state.tool_calls.drain(..) {
                    let input: Value = serde_json::from_str(&tc.arguments)
                        .unwrap_or(Value::Object(serde_json::Map::new()));
                    events.push(LlmEvent::ToolUse {
                        id: tc.id,
                        name: tc.name,
                        input,
                    });
                }
                events.push(LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                    usage: TokenUsage {
                        input_tokens: state.input_tokens,
                        output_tokens: state.output_tokens,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 0,
                    },
                });
            }
            "stop" => {
                events.push(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    usage: TokenUsage {
                        input_tokens: state.input_tokens,
                        output_tokens: state.output_tokens,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 0,
                    },
                });
            }
            "length" => {
                events.push(LlmEvent::Done {
                    stop_reason: StopReason::MaxTokens,
                    usage: TokenUsage {
                        input_tokens: state.input_tokens,
                        output_tokens: state.output_tokens,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 0,
                    },
                });
            }
            _ => {}
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::message::*;

    fn no_compat() -> ProviderCompat {
        ProviderCompat::default()
    }

    fn openai_compat() -> ProviderCompat {
        ProviderCompat::openai_defaults()
    }

    // --- max_tokens_field ---

    #[test]
    fn test_max_tokens_field_default() {
        let provider = OpenAIProvider::new("key", "http://localhost", openai_compat());
        let req = LlmRequest {
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
            thinking: None,
            reasoning_effort: None,
        };
        let body = provider.build_request_body(&req);
        assert_eq!(body["max_tokens"], 1024);
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn test_max_tokens_field_custom() {
        let compat = ProviderCompat {
            max_tokens_field: Some("max_completion_tokens".into()),
            ..Default::default()
        };
        let provider = OpenAIProvider::new("key", "http://localhost", compat);
        let req = LlmRequest {
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 2048,
            thinking: None,
            reasoning_effort: None,
        };
        let body = provider.build_request_body(&req);
        assert_eq!(body["max_completion_tokens"], 2048);
        assert!(body.get("max_tokens").is_none());
    }

    // --- merge_assistant_messages ---

    #[test]
    fn test_merge_assistant_messages_enabled() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text { text: "hello".into() }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text { text: " world".into() }],
            },
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 1);
        assert_eq!(assistant_msgs[0]["content"], "hello world");
    }

    #[test]
    fn test_merge_assistant_messages_disabled() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text { text: "hello".into() }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text { text: " world".into() }],
            },
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &no_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 2);
    }

    // --- clean_orphan_tool_calls ---

    #[test]
    fn test_clean_orphan_tool_calls_enabled() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "tc1".into(),
                        name: "bash".into(),
                        input: json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "tc2".into(),
                        name: "read".into(),
                        input: json!({}),
                    },
                ],
            },
            Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            },
            // tc2 has no result → orphan
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["id"], "tc1");
    }

    #[test]
    fn test_clean_orphan_tool_calls_disabled() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "tc1".into(),
                        name: "bash".into(),
                        input: json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "tc2".into(),
                        name: "read".into(),
                        input: json!({}),
                    },
                ],
            },
            Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            },
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &no_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 2);
    }

    // --- dedup_tool_results ---

    #[test]
    fn test_dedup_tool_results_enabled() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "tc1".into(),
                    name: "bash".into(),
                    input: json!({}),
                }],
            },
            Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "first".into(),
                    is_error: false,
                }],
            },
            Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "second".into(),
                    is_error: false,
                }],
            },
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["content"], "second");
    }

    #[test]
    fn test_dedup_tool_results_disabled() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "tc1".into(),
                    name: "bash".into(),
                    input: json!({}),
                }],
            },
            Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "first".into(),
                    is_error: false,
                }],
            },
            Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "second".into(),
                    is_error: false,
                }],
            },
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &no_compat());
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 2);
    }

    // --- strip_patterns ---

    #[test]
    fn test_strip_patterns() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello __MARKER__ world".into(),
            }],
        }];
        let compat = ProviderCompat {
            strip_patterns: Some(vec!["__MARKER__".into()]),
            ..Default::default()
        };
        let result = OpenAIProvider::build_messages(&messages, "", &compat);
        assert_eq!(result[0]["content"], "hello  world");
    }
}
