use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;

use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{StopReason, TokenUsage};

use crate::composed::ComposedProvider;
use crate::openai_messages::generate_call_id;
use crate::transport::{OpenAiTransport, ProviderTransport};
use crate::{LlmProvider, ProviderError};
use aion_config::compat::ProviderCompat;

pub struct OpenAIProvider {
    inner: ComposedProvider,
}

impl OpenAIProvider {
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat) -> Self {
        let transport = ProviderTransport::OpenAi(OpenAiTransport::new(api_key, base_url));
        let inner = ComposedProvider::new(transport, compat.clone());

        Self { inner }
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
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

pub(crate) fn parse_sse_chunk(data: &str, state: &mut StreamState, auto_tool_id: bool) -> Vec<LlmEvent> {
    let mut events = Vec::new();

    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return events,
    };

    // Extract usage if present
    if let Some(usage) = json.get("usage") {
        let base_prompt = usage["prompt_tokens"].as_u64().unwrap_or(state.input_tokens);

        // DeepSeek-style: prompt_cache_hit_tokens is reported separately and
        // prompt_tokens only contains the cache-miss portion.
        // Add it to get the true total prompt size.
        let cache_hit = usage["prompt_cache_hit_tokens"].as_u64().unwrap_or(0);

        state.input_tokens = base_prompt + cache_hit;
        state.output_tokens = usage["completion_tokens"].as_u64().unwrap_or(state.output_tokens);
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
                        let input: Value =
                            serde_json::from_str(&tc.arguments).unwrap_or(Value::Object(serde_json::Map::new()));
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
        let chunk =
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":8,"completion_tokens":3}}"#;
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
        if let LlmEvent::ToolUse { id, name, input, .. } = &tool_events[0] {
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

        let chunk = r#"{"choices":[{"delta":{"content":"done"},"finish_reason":"stop","index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state, false);

        let text_events: Vec<_> = events.iter().filter(|e| matches!(e, LlmEvent::TextDelta(_))).collect();
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
            assert!(id.is_empty(), "id should remain empty when auto_tool_id is disabled");
        }
    }
}
