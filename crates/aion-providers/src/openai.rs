use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;

use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{StopReason, TokenUsage};

use crate::composed::ComposedProvider;
use crate::openai_messages::generate_call_id;
use crate::stream_diagnostics::{OpenAiStreamDiagnostics, StreamTermination};
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
    cache_read_tokens: u64,
    diagnostics: OpenAiStreamDiagnostics,
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
            cache_read_tokens: 0,
            diagnostics: OpenAiStreamDiagnostics::default(),
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
                    cache_read_tokens: self.cache_read_tokens,
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

    pub(crate) fn diagnostics_mut(&mut self) -> &mut OpenAiStreamDiagnostics {
        &mut self.diagnostics
    }

    pub(crate) fn emit_diagnostics(&self, termination: StreamTermination, duration: Duration) {
        self.diagnostics
            .emit_summary(termination, duration, self.input_tokens, self.output_tokens);
    }
}

pub(crate) fn parse_sse_chunk(data: &str, state: &mut StreamState, auto_tool_id: bool) -> Vec<LlmEvent> {
    let mut events = Vec::new();

    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => {
            state.diagnostics.observe_invalid_json();
            return events;
        }
    };
    state.diagnostics.observe_json(&json);

    // Extract usage if present
    if let Some(usage) = json.get("usage") {
        state.input_tokens = usage["prompt_tokens"].as_u64().unwrap_or(state.input_tokens);
        state.output_tokens = usage["completion_tokens"].as_u64().unwrap_or(state.output_tokens);
        state.cache_read_tokens = usage
            .pointer("/prompt_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage["cached_tokens"].as_u64())
            .or_else(|| usage["prompt_cache_hit_tokens"].as_u64())
            .unwrap_or(state.cache_read_tokens);
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
#[path = "openai_test.rs"]
mod openai_test;
