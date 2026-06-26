use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use tokio::sync::mpsc;

use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use aion_types::tool::{ToolDef, truncate_deferred_description};

use crate::framing::{FrameKind, SseLineFramer};
use crate::parser::{OpenAiParser, ResponseParser};
use crate::projector::{OpenAiProjector, projection_to_provider_error};
use crate::stream_runner::{RetryPolicy, StreamOutcome, run_stream};
use crate::tool_call_sanitize::{DroppedToolCallReason, format_dropped_tool_call};
use crate::{LlmProvider, ProviderError};
use aion_config::compat::ProviderCompat;

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

    fn build_headers(&self) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        let bearer = format!("Bearer {}", self.api_key);
        let auth = HeaderValue::from_str(&bearer).map_err(|e| {
            ProviderError::Connection(format!("Invalid authorization header: {}", e))
        })?;
        headers.insert(AUTHORIZATION, auth);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    pub(crate) fn build_messages(
        messages: &[Message],
        system: &str,
        compat: &ProviderCompat,
    ) -> Vec<Value> {
        let mut result: Vec<Value> = Vec::new();
        let sanitize = compat.sanitize_malformed_tool_calls();
        let auto_tool_id = compat.auto_tool_id();
        let clean_orphan_tool_results = compat.clean_orphan_tool_results();
        // tool_call ids dropped as malformed; their paired tool results must be
        // skipped later to avoid orphan "tool" messages.
        let mut dropped_ids: HashMap<String, VecDeque<DroppedToolCallReason>> = HashMap::new();
        let mut available_tool_call_ids: HashSet<String> = HashSet::new();
        let mut generated_tool_call_ids: HashMap<String, VecDeque<String>> = HashMap::new();

        // Check if any assistant message in the conversation has thinking content.
        // If so, DeepSeek API requires ALL assistant messages to include
        // reasoning_content (even if empty string).
        let has_any_thinking = messages.iter().any(|m| {
            m.role == Role::Assistant
                && m.content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Thinking { .. }))
        });

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
                                if let Some(reasons) = dropped_ids.get_mut(tool_use_id)
                                    && reasons.pop_front().is_some()
                                {
                                    continue;
                                }
                                let projected_tool_use_id = generated_tool_call_ids
                                    .get_mut(tool_use_id)
                                    .and_then(VecDeque::pop_front)
                                    .unwrap_or_else(|| tool_use_id.clone());
                                if clean_orphan_tool_results
                                    && !available_tool_call_ids.contains(&projected_tool_use_id)
                                {
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

                    // Preserve reasoning_content for models with thinking mode
                    // (e.g. DeepSeek Reasoner, Kimi K2.5). The API requires
                    // ALL assistant messages to include reasoning_content once
                    // any message in the conversation has it.
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

                    if has_any_thinking {
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
                        if let ContentBlock::ToolUse {
                            id,
                            name,
                            input,
                            extra,
                        } = b
                        {
                            if sanitize && name.is_empty() {
                                dropped_ids
                                    .entry(id.clone())
                                    .or_default()
                                    .push_back(DroppedToolCallReason::EmptyName);
                                dropped_lines.push(format_dropped_tool_call(
                                    DroppedToolCallReason::EmptyName,
                                    input,
                                ));
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
                                dropped_lines.push(format_dropped_tool_call(
                                    DroppedToolCallReason::EmptyId,
                                    input,
                                ));
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
                            tool_use_id,
                            content,
                            ..
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
                            if clean_orphan_tool_results
                                && !available_tool_call_ids.contains(&projected_tool_use_id)
                            {
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

    pub(crate) fn build_tools(tools: &[ToolDef]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                if t.deferred {
                    let short_desc = truncate_deferred_description(&t.description);
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": format!(
                                "(Deferred) {short_desc} — Use ToolSearch to load full schema before calling."
                            ),
                            "parameters": {
                                "type": "object",
                                "properties": {}
                            }
                        }
                    })
                } else {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema
                        }
                    })
                }
            })
            .collect()
    }

    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, ProviderError> {
        OpenAiProjector::project(request, &self.compat).map_err(projection_to_provider_error)
    }
}

/// Generate a unique tool call ID in OpenAI `call_xxx` format.
fn generate_call_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let rand: u64 = (ts as u64).wrapping_mul(6364136223846793005);
    format!("call_{:016x}", rand)
}

/// Strip configured patterns from text content
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

/// Deduplicate tool results: keep last occurrence of each tool_call_id
fn dedup_tool_results(messages: &mut Vec<Value>) {
    use std::collections::HashMap;

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

/// Remove tool_call entries from assistant messages that have no corresponding tool result
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
                tc["id"]
                    .as_str()
                    .map(|id| answered_ids.contains(id))
                    .unwrap_or(true)
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

            // Merge reasoning_content
            let curr_rc = messages[i]["reasoning_content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let next_rc = next["reasoning_content"].as_str().unwrap_or("").to_string();
            let merged_rc = match (curr_rc.is_empty(), next_rc.is_empty()) {
                (true, true) => String::new(),
                (true, false) => next_rc,
                (false, true) => curr_rc,
                (false, false) => format!("{}{}", curr_rc, next_rc),
            };

            if !merged_rc.is_empty() {
                messages[i]["reasoning_content"] = json!(merged_rc);
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

/// State for accumulating tool call deltas by index
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
    extra: Option<Value>,
}

pub(crate) struct StreamState {
    tool_calls: Vec<ToolCallAccumulator>,
    input_tokens: u64,
    output_tokens: u64,
    /// Deferred Done event: populated when finish_reason arrives, emitted on
    /// [DONE] so the final usage-only chunk has a chance to update token counts.
    pending_done: Option<LlmEvent>,
}

impl StreamState {
    pub(crate) fn new() -> Self {
        Self {
            tool_calls: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            pending_done: None,
        }
    }

    /// Emit the deferred Done event with up-to-date token counts.
    ///
    /// OpenAI sends usage in a separate trailing chunk (choices:[]) *after* the
    /// chunk that carries `finish_reason`. We defer the Done event until [DONE]
    /// so that token counts are always accurate.
    pub(crate) fn flush_done(&mut self) -> Option<LlmEvent> {
        let pending = self.pending_done.take()?;
        Some(match pending {
            LlmEvent::Done { stop_reason, .. } => LlmEvent::Done {
                stop_reason,
                usage: TokenUsage {
                    input_tokens: self.input_tokens,
                    output_tokens: self.output_tokens,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
            other => other,
        })
    }

    fn get_or_create_tool(&mut self, index: usize) -> &mut ToolCallAccumulator {
        while self.tool_calls.len() <= index {
            self.tool_calls.push(ToolCallAccumulator {
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
                extra: None,
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
        let body = self.build_request_body(request)?;
        let headers = self.build_headers()?;

        tracing::debug!(target: "aion_providers", body = %serde_json::to_string_pretty(&body).unwrap_or_default(), "outgoing request");

        let auto_tool_id = self.compat.auto_tool_id();
        let client = self.client.clone();

        let send = move || {
            send_openai_stream_request(client.clone(), url.clone(), headers.clone(), body.clone())
        };
        let process = move |response, tx| async move {
            process_sse_stream(response, &tx, auto_tool_id).await
        };

        run_stream(
            send,
            process,
            RetryPolicy::new(crate::retry::MAX_STREAM_RETRIES, true, true),
        )
        .await
    }
}

async fn send_openai_stream_request(
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    body: Value,
) -> Result<reqwest::Response, ProviderError> {
    let response = client
        .post(&url)
        .headers(headers)
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

    Ok(response)
}

async fn process_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
    auto_tool_id: bool,
) -> StreamOutcome {
    use futures::StreamExt;

    let parser = OpenAiParser { auto_tool_id };
    let mut state = parser.new_state();
    let mut framer = SseLineFramer::default();
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
        for frame in framer.push_text(&text, "[DONE]") {
            tracing::debug!(target: "aion_providers", chunk = %frame.data, "sse chunk received");
            let is_done = frame.kind == FrameKind::Done;
            let events = parser.parse_frame(&frame, &mut state);
            for event in events {
                if matches!(
                    event,
                    LlmEvent::TextDelta(_) | LlmEvent::ThinkingDelta(_) | LlmEvent::ToolUse { .. }
                ) {
                    emitted_content = true;
                }
                if tx.send(event).await.is_err() {
                    return StreamOutcome::Ok;
                }
            }
            if is_done {
                return StreamOutcome::Ok;
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

pub(crate) fn parse_sse_chunk(
    data: &str,
    state: &mut StreamState,
    auto_tool_id: bool,
) -> Vec<LlmEvent> {
    let mut events = Vec::new();

    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return events,
    };

    // Extract usage if present
    if let Some(usage) = json.get("usage") {
        let base_prompt = usage["prompt_tokens"]
            .as_u64()
            .unwrap_or(state.input_tokens);

        // DeepSeek-style: prompt_cache_hit_tokens is reported separately and
        // prompt_tokens only contains the cache-miss portion.
        // Add it to get the true total prompt size.
        let cache_hit = usage["prompt_cache_hit_tokens"].as_u64().unwrap_or(0);

        state.input_tokens = base_prompt + cache_hit;
        state.output_tokens = usage["completion_tokens"]
            .as_u64()
            .unwrap_or(state.output_tokens);
    }

    let Some(choice) = json["choices"].as_array().and_then(|c| c.first()) else {
        return events;
    };

    let delta = &choice["delta"];

    // Reasoning content (OpenAI reasoning models)
    if let Some(reasoning) = delta["reasoning_content"].as_str()
        && !reasoning.is_empty()
    {
        events.push(LlmEvent::ThinkingDelta(reasoning.to_string()));
    }

    // Text content
    if let Some(content) = delta["content"].as_str()
        && !content.is_empty()
    {
        events.push(LlmEvent::TextDelta(content.to_string()));
    }

    // Tool calls
    if let Some(tool_calls) = delta["tool_calls"].as_array() {
        for tc in tool_calls {
            let index = tc["index"].as_u64().unwrap_or(0) as usize;
            let acc = state.get_or_create_tool(index);

            if let Some(id) = tc["id"].as_str() {
                acc.id = id.to_string();
            }
            // Only overwrite when non-empty — some third-party APIs send `"name":""`
            // in every delta chunk which would erase the real name from the first chunk.
            if let Some(name) = tc["function"]["name"].as_str().filter(|n| !n.is_empty()) {
                acc.name = name.to_string();
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                acc.arguments.push_str(args);
            }
            if let Some(extra) = tc.get("extra_content").filter(|v| !v.is_null()) {
                acc.extra = Some(extra.clone());
            }
        }
    }

    // Check finish_reason — defer Done until [DONE] so the trailing usage
    // chunk (choices:[]) can update token counts first.
    if let Some(finish_reason) = choice["finish_reason"].as_str() {
        match finish_reason {
            "tool_calls" | "stop" => {
                if !state.tool_calls.is_empty() {
                    // Emit accumulated tool calls. Gemini uses "stop" instead of
                    // "tool_calls" as finish_reason, so we handle both here.
                    for tc in state.tool_calls.drain(..) {
                        let id = if tc.id.is_empty() && auto_tool_id {
                            generate_call_id()
                        } else {
                            tc.id
                        };
                        let input: Value = serde_json::from_str(&tc.arguments)
                            .unwrap_or(Value::Object(serde_json::Map::new()));
                        if tc.name.is_empty() {
                            tracing::warn!(
                                target: "aion_providers",
                                tool_call_id = %id,
                                "provider emitted tool_call with empty function name; recorded to history as-is"
                            );
                        }
                        events.push(LlmEvent::ToolUse {
                            id,
                            name: tc.name,
                            input,
                            extra: tc.extra,
                        });
                    }
                    state.pending_done = Some(LlmEvent::Done {
                        stop_reason: StopReason::ToolUse,
                        usage: TokenUsage::default(),
                    });
                } else if finish_reason == "stop" {
                    state.pending_done = Some(LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        usage: TokenUsage::default(),
                    });
                } else {
                    // "tool_calls" with empty accumulator — shouldn't happen,
                    // but treat as ToolUse for safety.
                    state.pending_done = Some(LlmEvent::Done {
                        stop_reason: StopReason::ToolUse,
                        usage: TokenUsage::default(),
                    });
                }
            }
            "length" => {
                state.pending_done = Some(LlmEvent::Done {
                    stop_reason: StopReason::MaxTokens,
                    usage: TokenUsage::default(),
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
    use aion_config::compat::TransportCompat;

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
        let body = provider
            .build_request_body(&req)
            .expect("request body projection should succeed");
        assert_eq!(body["max_tokens"], 1024);
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn test_max_tokens_field_custom() {
        let compat = ProviderCompat {
            transport: TransportCompat {
                max_tokens_field: Some("max_completion_tokens".into()),
                ..Default::default()
            },
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
        let body = provider
            .build_request_body(&req)
            .expect("request body projection should succeed");
        assert_eq!(body["max_completion_tokens"], 2048);
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn test_projection_limit_maps_to_non_retryable_prompt_too_long() {
        let mut compat = ProviderCompat::openai_defaults();
        compat.tools.max_tool_count = Some(0);
        let provider = OpenAIProvider::new("key", "http://localhost", compat);
        let req = LlmRequest {
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![ToolDef {
                name: "read".into(),
                description: "Read".into(),
                input_schema: json!({"type":"object","properties":{}}),
                deferred: false,
            }],
            max_tokens: 1024,
            thinking: None,
            reasoning_effort: None,
        };

        let error = provider
            .build_request_body(&req)
            .expect_err("projection limit should map to provider error");

        match &error {
            ProviderError::PromptTooLong(message) => {
                assert!(message.contains("openai tools count 1 exceeds configured limit 0"));
            }
            other => panic!("unexpected provider error: {other}"),
        }
        assert!(!error.is_retryable());
    }

    // --- merge_assistant_messages ---

    #[test]
    fn test_merge_assistant_messages_enabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: " world".into(),
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 1);
        assert_eq!(assistant_msgs[0]["content"], "hello world");
    }

    #[test]
    fn test_merge_assistant_messages_disabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: " world".into(),
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &no_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 2);
    }

    // --- clean_orphan_tool_calls ---

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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
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
        let result = OpenAIProvider::build_messages(&messages, "", &no_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 2);
    }

    // F2-1
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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        assert!(result.iter().all(|m| m["role"] != "tool"));
    }

    // F2-3
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
        let result = OpenAIProvider::build_messages(&messages, "", &compat);
        assert!(result.iter().any(|m| {
            m["role"] == "tool" && m["tool_call_id"] == "missing" && m["content"] == "orphan"
        }));
    }

    // F2-4
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

        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["id"], "matched");
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["tool_call_id"], "matched");
    }

    // H2-1
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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        assert!(
            result
                .iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == "call_x")
        );
    }

    // F2-5
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
        let result = OpenAIProvider::build_messages(&messages, "", &compat);
        assert!(result.iter().all(|m| m["role"] != "tool"));
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert!(assistant.get("tool_calls").is_none());
        let content = assistant["content"].as_str().unwrap();
        assert!(content.contains("[tool call skipped:"));
        assert!(content.contains("empty tool call id"));
        assert!(content.contains("arguments={\"command\":\"ls\"}"));
    }

    // F2-6
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
        let result = OpenAIProvider::build_messages(&messages, "", &compat);
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
        let result = OpenAIProvider::build_messages(&messages, "", &compat);
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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
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
        let result = OpenAIProvider::build_messages(&messages, "", &compat);
        let assistant_with_call = result
            .iter()
            .find(|m| {
                m["tool_calls"]
                    .as_array()
                    .is_some_and(|calls| !calls.is_empty())
            })
            .unwrap();
        let generated_id = assistant_with_call["tool_calls"][0]["id"].as_str().unwrap();
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["tool_call_id"], generated_id);
        assert_eq!(tool_msgs[0]["content"], "ok");
    }

    // F1-1
    #[test]
    fn test_empty_name_toolcall_downgraded_and_paired_result_dropped() {
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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        // no role:"tool" orphan survives
        assert!(
            result.iter().all(|m| m["role"] != "tool"),
            "paired tool result must be dropped"
        );
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        // no tool_calls with empty name
        let has_empty = assistant
            .get("tool_calls")
            .and_then(|t| t.as_array())
            .map(|a| a.iter().any(|tc| tc["function"]["name"] == ""))
            .unwrap_or(false);
        assert!(!has_empty, "no empty-name tool_call in projection");
        // downgrade text present in content
        assert!(
            assistant["content"]
                .as_str()
                .unwrap()
                .contains("[tool call skipped:")
        );
        assert!(assistant["content"].as_str().unwrap().contains("writing"));
    }

    // F1-7
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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1); // only Bash kept
        assert_eq!(tcs[0]["function"]["name"], "Bash");
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1); // only ok's result kept
        assert_eq!(tool_msgs[0]["tool_call_id"], "ok");
    }

    // F1-3
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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        assert!(result.iter().all(|m| m["role"] != "tool"));
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert!(
            assistant["content"]
                .as_str()
                .unwrap()
                .contains("[tool call skipped:")
        );
    }

    // F1-5
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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let content = result.iter().find(|m| m["role"] == "assistant").unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(content.matches("[tool call skipped:").count(), 2);
        assert!(content.contains("{\"x\":1}") && content.contains("{\"y\":2}"));
    }

    // F1-14
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
        let result = OpenAIProvider::build_messages(&messages, "", &compat);
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], ""); // raw empty name preserved
    }

    // H1-1
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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "Bash");
        assert!(
            result
                .iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == "call_x")
        );
    }

    // --- dedup_tool_results ---

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
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["content"], "second");
    }

    // --- usage token parsing ---

    #[test]
    fn test_usage_from_trailing_chunk() {
        // OpenAI sends usage in a trailing chunk where choices:[] — the Done
        // event must carry the token counts from that chunk, not zeros.
        let mut state = StreamState::new();

        // chunk 1: finish_reason + text delta, no usage
        let chunk1 = r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}"#;
        let events = parse_sse_chunk(chunk1, &mut state, false);
        // TextDelta is emitted immediately; Done is deferred.
        assert!(
            events.iter().all(|e| !matches!(e, LlmEvent::Done { .. })),
            "Done should be deferred, not emitted with finish_reason chunk"
        );
        assert!(state.pending_done.is_some());

        // chunk 2: trailing usage-only chunk (choices:[])
        let chunk2 = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let events2 = parse_sse_chunk(chunk2, &mut state, false);
        assert!(events2.is_empty());
        assert_eq!(state.input_tokens, 10);
        assert_eq!(state.output_tokens, 5);

        // [DONE] — flush with final counts
        let done = state.flush_done().expect("pending_done should be Some");
        match done {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 5);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_usage_in_finish_chunk() {
        // Some providers/models include usage in the same chunk as finish_reason.
        // Counts should still be correct after flush.
        let mut state = StreamState::new();

        // No text delta here, only finish_reason + usage in the same chunk.
        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":8,"completion_tokens":3}}"#;
        let events = parse_sse_chunk(chunk, &mut state, false);
        assert!(
            events.iter().all(|e| !matches!(e, LlmEvent::Done { .. })),
            "Done should be deferred even when usage is in the finish chunk"
        );
        assert_eq!(state.output_tokens, 3);

        let done = state.flush_done().unwrap();
        match done {
            LlmEvent::Done { usage, .. } => {
                assert_eq!(usage.output_tokens, 3);
            }
            other => panic!("expected Done, got {other:?}"),
        }
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
        let result = OpenAIProvider::build_tools(&tools);

        // Core tool has full parameters
        let read_params = &result[0]["function"]["parameters"];
        assert!(read_params["properties"].get("path").is_some());

        // Deferred tool has empty parameters and modified description
        let spawn_params = &result[1]["function"]["parameters"];
        assert!(spawn_params["properties"].as_object().unwrap().is_empty());
        let spawn_desc = result[1]["function"]["description"].as_str().unwrap();
        assert!(spawn_desc.contains("ToolSearch"));
    }

    #[test]
    fn usage_includes_prompt_cache_hit_tokens() {
        // DeepSeek reports prompt_cache_hit_tokens separately;
        // input_tokens should be the sum of prompt_tokens + prompt_cache_hit_tokens
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":500,"completion_tokens":100,"prompt_cache_hit_tokens":999500}}"#;
        let _ = parse_sse_chunk(chunk, &mut state, false);

        assert_eq!(state.input_tokens, 1_000_000);
        assert_eq!(state.output_tokens, 100);
    }

    #[test]
    fn usage_with_prompt_tokens_details_cached() {
        // OpenAI standard: prompt_tokens already includes cached_tokens (it's the total)
        // prompt_tokens_details.cached_tokens is informational only
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1000000,"completion_tokens":100,"prompt_tokens_details":{"cached_tokens":999000}}}"#;
        let _ = parse_sse_chunk(chunk, &mut state, false);

        // prompt_tokens is already the full total for OpenAI
        assert_eq!(state.input_tokens, 1_000_000);
        assert_eq!(state.output_tokens, 100);
    }

    #[test]
    fn usage_without_cache_fields_unchanged() {
        // Provider that only sends prompt_tokens (no cache fields)
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":50000,"completion_tokens":200}}"#;
        let _ = parse_sse_chunk(chunk, &mut state, false);

        assert_eq!(state.input_tokens, 50_000);
        assert_eq!(state.output_tokens, 200);
    }

    #[test]
    fn tool_calls_with_stop_finish_reason() {
        // Gemini uses finish_reason:"stop" even when tool_calls are present.
        // The accumulated tool calls must still be emitted.
        let mut state = StreamState::new();

        // chunk 1: tool call delta (name + partial args)
        let chunk1 = r#"{"choices":[{"delta":{"role":"assistant","tool_calls":[{"extra_content":{},"function":{"arguments":"{\"skill\":\"test\",\"args\":\"hello\"}","name":"Skill"},"id":"call_abc123","type":"function"}]},"index":0}]}"#;
        let events1 = parse_sse_chunk(chunk1, &mut state, false);
        assert!(events1.is_empty(), "no events until finish_reason");
        assert_eq!(state.tool_calls.len(), 1);
        assert_eq!(state.tool_calls[0].name, "Skill");

        // chunk 2: finish_reason:"stop" (not "tool_calls")
        let chunk2 = r#"{"choices":[{"delta":{"role":"assistant"},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120}}"#;
        let events2 = parse_sse_chunk(chunk2, &mut state, false);

        // Tool call should be emitted
        let tool_events: Vec<_> = events2
            .iter()
            .filter(|e| matches!(e, LlmEvent::ToolUse { .. }))
            .collect();
        assert_eq!(tool_events.len(), 1, "tool call should be emitted on stop");
        if let LlmEvent::ToolUse {
            id, name, input, ..
        } = &tool_events[0]
        {
            assert_eq!(id, "call_abc123");
            assert_eq!(name, "Skill");
            assert_eq!(input["skill"], "test");
        }

        // Done should be deferred with ToolUse stop reason
        let done = state.flush_done().unwrap();
        match done {
            LlmEvent::Done { stop_reason, .. } => {
                assert_eq!(stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected Done with ToolUse, got {other:?}"),
        }

        assert!(state.tool_calls.is_empty(), "tool calls should be drained");
    }

    // F1-9
    #[test]
    fn test_empty_name_toolcall_still_emitted_to_history() {
        let mut state = StreamState::new();

        let chunk1 = r#"{"choices":[{"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_x","type":"function","function":{"name":"","arguments":"{}"}}]},"index":0}]}"#;
        let events1 = parse_sse_chunk(chunk1, &mut state, false);
        assert!(events1.is_empty(), "no events until finish_reason");

        let chunk2 = r#"{"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}]}"#;
        let events2 = parse_sse_chunk(chunk2, &mut state, false);

        let tool_use_name = events2.iter().find_map(|event| match event {
            LlmEvent::ToolUse { name, .. } => Some(name.clone()),
            _ => None,
        });

        assert_eq!(
            tool_use_name,
            Some(String::new()),
            "empty-name tool_call must still be emitted and recorded as-is"
        );
    }

    #[test]
    fn stop_without_tool_calls_unchanged() {
        // Standard stop without tool calls should still produce EndTurn.
        let mut state = StreamState::new();

        let chunk =
            r#"{"choices":[{"delta":{"content":"done"},"finish_reason":"stop","index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state, false);

        let text_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, LlmEvent::TextDelta(_)))
            .collect();
        assert_eq!(text_events.len(), 1);

        let done = state.flush_done().unwrap();
        match done {
            LlmEvent::Done { stop_reason, .. } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
            }
            other => panic!("expected Done with EndTurn, got {other:?}"),
        }
    }

    #[test]
    fn test_auto_tool_id_generates_id_when_empty() {
        let mut state = StreamState::new();

        // Simulate a provider that returns tool_calls without an id field
        let chunk = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"get_weather","arguments":"{\"city\":\"Beijing\"}"}}]},"finish_reason":"tool_calls","index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state, true);

        let tool_use = events
            .iter()
            .find(|e| matches!(e, LlmEvent::ToolUse { .. }))
            .expect("should emit ToolUse event");

        if let LlmEvent::ToolUse { id, name, .. } = tool_use {
            assert!(!id.is_empty(), "id should be auto-generated, not empty");
            assert!(id.starts_with("call_"), "id should have call_ prefix");
            assert_eq!(name, "get_weather");
        }
    }

    #[test]
    fn test_auto_tool_id_preserves_existing_id() {
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_existing_123","function":{"name":"read_file","arguments":"{}"}}]},"finish_reason":"tool_calls","index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state, true);

        let tool_use = events
            .iter()
            .find(|e| matches!(e, LlmEvent::ToolUse { .. }))
            .expect("should emit ToolUse event");

        if let LlmEvent::ToolUse { id, .. } = tool_use {
            assert_eq!(id, "call_existing_123", "existing id should be preserved");
        }
    }

    #[test]
    fn test_auto_tool_id_disabled_keeps_empty() {
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"get_weather","arguments":"{}"}}]},"finish_reason":"tool_calls","index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state, false);

        let tool_use = events
            .iter()
            .find(|e| matches!(e, LlmEvent::ToolUse { .. }))
            .expect("should emit ToolUse event");

        if let LlmEvent::ToolUse { id, .. } = tool_use {
            assert!(
                id.is_empty(),
                "id should remain empty when auto_tool_id is disabled"
            );
        }
    }

    // --- Golden body snapshots (baseline for compat-split / seam-extraction refactors) ---

    fn golden_provider(compat: ProviderCompat) -> OpenAIProvider {
        OpenAIProvider::new("test-key", "https://example.test/v1", compat)
    }

    fn golden_req(messages: Vec<Message>, tools: Vec<ToolDef>) -> LlmRequest {
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

    #[test]
    fn golden_openai_basic() {
        let provider = golden_provider(ProviderCompat::openai_defaults());
        let request = golden_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            vec![],
        );
        let body = provider
            .build_request_body(&request)
            .expect("request body projection should succeed");
        insta::assert_json_snapshot!("openai_basic", body);
    }

    fn sample_tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "read".to_string(),
                description: "Read a file".to_string(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
                deferred: false,
            },
            ToolDef {
                name: "list".to_string(),
                description: "List dir".to_string(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: false,
            },
        ]
    }

    #[test]
    fn golden_openai_with_tools() {
        let provider = golden_provider(ProviderCompat::openai_defaults());
        let request = golden_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "go".to_string(),
                }],
            )],
            sample_tools(),
        );
        insta::assert_json_snapshot!(
            "openai_with_tools",
            provider
                .build_request_body(&request)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_openai_with_tool_result() {
        let provider = golden_provider(ProviderCompat::openai_defaults());
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    input: json!({"path": "a.txt"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "file contents".to_string(),
                    is_error: false,
                }],
            ),
        ];
        insta::assert_json_snapshot!(
            "openai_with_tool_result",
            provider
                .build_request_body(&golden_req(messages, vec![]))
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_openai_with_thinking() {
        let provider = golden_provider(ProviderCompat::openai_defaults());
        let messages = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "q1".to_string(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Thinking {
                        thinking: "let me think".to_string(),
                        signature: None,
                    },
                    ContentBlock::Text {
                        text: "answer".to_string(),
                    },
                ],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "q2".to_string(),
                }],
            ),
        ];
        insta::assert_json_snapshot!(
            "openai_with_thinking",
            provider
                .build_request_body(&golden_req(messages, vec![]))
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_openai_with_reasoning_effort() {
        let provider = golden_provider(ProviderCompat::openai_defaults());
        let mut request = golden_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
            )],
            vec![],
        );
        request.reasoning_effort = Some("medium".to_string());
        insta::assert_json_snapshot!(
            "openai_with_reasoning_effort",
            provider
                .build_request_body(&request)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_openai_custom_max_tokens_field() {
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.max_tokens_field = Some("max_completion_tokens".to_string());
        let provider = golden_provider(compat);
        let request = golden_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
            )],
            vec![],
        );
        insta::assert_json_snapshot!(
            "openai_custom_max_tokens_field",
            provider
                .build_request_body(&request)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_openai_field_controls_disabled() {
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.include_stream_options = Some(false);
        compat.tools.emit_tools = Some(false);
        compat.reasoning.supports_effort = Some(false);
        let provider = golden_provider(compat);
        let mut request = golden_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
            )],
            sample_tools(),
        );
        request.reasoning_effort = Some("medium".to_string());

        insta::assert_json_snapshot!(
            "openai_field_controls_disabled",
            provider
                .build_request_body(&request)
                .expect("request body projection should succeed")
        );
    }
}
