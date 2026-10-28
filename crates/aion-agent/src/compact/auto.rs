//! Autocompact: context-threshold-triggered LLM summarization.
//!
//! When the best-known context size exceeds the configured threshold, this module
//! calls the LLM to produce a structured summary of the conversation,
//! then replaces the full history with a compact boundary marker and the
//! summary.  A circuit breaker prevents runaway retries.

use aion_config::compact::CompactConfig;
use aion_providers::{LlmProvider, ProviderError};
use aion_types::compact::{CompactMetadata, CompactTrigger};
use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{ContentBlock, Message, Role, TokenUsage};
use tokio::sync::mpsc;

use super::estimate::estimate_messages_tokens;
use super::prompt::{
    COMPACT_MAX_OUTPUT_TOKENS, COMPACT_SYSTEM_PROMPT, build_compact_prompt, build_summary_content,
    format_compact_summary,
};
use super::state::CompactState;

/// Maximum number of prompt-too-long retries.
const MAX_PTL_RETRIES: u32 = 2;

/// Content prefix for the compact boundary marker message.
pub const BOUNDARY_PREFIX: &str = "[Conversation compacted]";

// ── Public types ────────────────────────────────────────────────────────────

/// Result of a successful autocompact operation.
#[derive(Debug, Clone)]
pub struct CompactResult {
    /// Post-compact messages that replace the original conversation.
    /// Contains a boundary marker, a summary message, and optionally a
    /// verbatim tail of the most recent turns.
    pub messages: Vec<Message>,
    /// How many original messages were summarized (the verbatim tail is
    /// excluded from this count).
    pub messages_summarized: usize,
    /// Best-known context token count before compaction.
    pub pre_compact_tokens: u64,
}

/// Errors specific to autocompact.
#[derive(Debug, thiserror::Error)]
pub enum CompactError {
    #[error("LLM provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("Prompt too long after {attempts} retries")]
    PromptTooLong { attempts: u32 },
    #[error("Empty response from LLM")]
    EmptyResponse,
    #[error("Stream error: {0}")]
    StreamError(String),
    #[error("Circuit breaker tripped after {failures} consecutive failures")]
    CircuitBroken { failures: u32 },
}

// ── Trigger check ───────────────────────────────────────────────────────────

/// Check if autocompact should trigger based on the best-known context size.
///
/// When `autocompact_threshold_pct` is set, threshold = context_window * pct / 100.
/// Otherwise falls back to: `threshold = context_window - output_reserve - autocompact_buffer`
pub fn should_autocompact(context_tokens: u64, config: &CompactConfig) -> bool {
    if !config.enabled {
        return false;
    }
    let threshold = if let Some(pct) = config.autocompact_threshold_pct {
        config.context_window * pct as usize / 100
    } else {
        let effective_window = config.context_window.saturating_sub(config.output_reserve);
        effective_window.saturating_sub(config.autocompact_buffer)
    };
    context_tokens as usize >= threshold
}

// ── Core autocompact ────────────────────────────────────────────────────────

/// Execute autocompact: call LLM to summarize the conversation.
///
/// 1. Build a summary prompt and send conversation + prompt to the LLM.
/// 2. If the prompt is too long, truncate oldest 20% messages and retry
///    (up to [`MAX_PTL_RETRIES`] times).
/// 3. Parse the `<summary>` from the response.
/// 4. Return a [`CompactResult`] with boundary marker + summary messages.
///
/// On failure, increments `state.consecutive_failures`.
/// On success, resets the failure counter.
pub async fn autocompact(
    provider: &dyn LlmProvider,
    messages: &[Message],
    model: &str,
    config: &CompactConfig,
    state: &mut CompactState,
) -> Result<CompactResult, CompactError> {
    // Circuit breaker check
    if state.is_circuit_broken(config) {
        return Err(CompactError::CircuitBroken {
            failures: state.consecutive_failures,
        });
    }

    let pre_compact_tokens = state.last_input_tokens;

    // Build messages for the compact LLM call: conversation + summary prompt.
    // The conversation is trimmed up front: this request re-sends the whole
    // history plus a fixed 20K completion budget, so a conversation that merely
    // sits near the window produces a request the provider rejects. Trimming
    // here is what makes compaction work against OpenAI-compatible servers,
    // which answer a plain 400 instead of a recognizable prompt-too-long error.
    let (mut conv_messages, trimmed) = fit_to_window(messages.to_vec(), config.context_window);
    let mut prompt = build_compact_prompt();
    if trimmed {
        prompt.push_str(TRUNCATED_NOTICE);
    }
    conv_messages.push(Message::new(Role::User, vec![ContentBlock::Text { text: prompt }]));

    let mut ptl_attempts = 0u32;

    let summary_text = loop {
        let request = LlmRequest {
            model: model.to_string(),
            system: COMPACT_SYSTEM_PROMPT.to_string(),
            messages: conv_messages.clone(),
            tools: vec![],
            max_tokens: Some(COMPACT_MAX_OUTPUT_TOKENS),
            thinking: None,
            reasoning_effort: None,
        };

        match provider.stream(&request).await {
            Ok(rx) => match collect_stream_text(rx).await {
                Ok((text, _usage)) => break text,
                Err(e) => {
                    state.record_failure();
                    return Err(e);
                }
            },
            Err(ProviderError::PromptTooLong(_)) if ptl_attempts < MAX_PTL_RETRIES => {
                ptl_attempts += 1;
                // Remove the summary prompt (last msg), truncate, re-add prompt
                let conversation_part = &conv_messages[..conv_messages.len() - 1];
                match truncate_for_retry(conversation_part) {
                    Some(mut truncated) => {
                        truncated.push(Message::new(
                            Role::User,
                            vec![ContentBlock::Text {
                                text: build_compact_prompt(),
                            }],
                        ));
                        conv_messages = truncated;
                    }
                    None => {
                        state.record_failure();
                        return Err(CompactError::PromptTooLong { attempts: ptl_attempts });
                    }
                }
            }
            Err(ProviderError::PromptTooLong(_)) => {
                state.record_failure();
                return Err(CompactError::PromptTooLong { attempts: ptl_attempts });
            }
            Err(e) => {
                state.record_failure();
                return Err(CompactError::Provider(e));
            }
        }
    };

    if summary_text.trim().is_empty() {
        state.record_failure();
        return Err(CompactError::EmptyResponse);
    }

    // Format and build post-compact messages
    let formatted = format_compact_summary(&summary_text);
    let summary_content = build_summary_content(&formatted, true);
    let summary_msg = Message::new(Role::User, vec![ContentBlock::Text { text: summary_content }]);

    // Keep the most recent turns verbatim after the summary so fine-grained
    // recent context is not lost to summarization. The tail is bounded by the
    // post-compact input budget and `keep_recent_tokens`.
    let post_budget = (config.context_window as u64).saturating_sub(config.output_reserve as u64);
    let fixed_tokens =
        estimate_messages_tokens(std::slice::from_ref(&summary_msg)).saturating_add(COMPACT_PROMPT_RESERVE);
    let tail_budget = (config.keep_recent_tokens as u64).min(post_budget.saturating_sub(fixed_tokens));
    let tail_start = select_recent_tail(messages, tail_budget);
    let tail = &messages[tail_start..];
    let messages_summarized = messages.len().saturating_sub(tail.len());

    let metadata = CompactMetadata {
        trigger: CompactTrigger::Auto,
        pre_compact_tokens,
        messages_summarized,
    };

    let boundary_text = format!(
        "{BOUNDARY_PREFIX}\n{}",
        serde_json::to_string(&metadata).expect("CompactMetadata serialization cannot fail")
    );

    let boundary_msg = Message::new(Role::User, vec![ContentBlock::Text { text: boundary_text }]);

    let mut post_messages = Vec::with_capacity(tail.len() + 2);
    post_messages.push(boundary_msg);
    post_messages.push(summary_msg);
    post_messages.extend_from_slice(tail);

    state.record_success();

    Ok(CompactResult {
        messages: post_messages,
        messages_summarized,
        pre_compact_tokens,
    })
}

/// Index of the first message kept verbatim as the post-compact tail.
///
/// Walks backwards from the newest message while the accumulated estimate fits
/// `max_tokens`, then moves the cut forward to a genuine user turn so a
/// tool-call / tool-result pair is never split across the compaction boundary.
/// Returns `messages.len()` (empty tail) when no verbatim tail fits.
fn select_recent_tail(messages: &[Message], max_tokens: u64) -> usize {
    if max_tokens == 0 || messages.is_empty() {
        return messages.len();
    }

    let mut used = 0_u64;
    let mut start = messages.len();
    while start > 0 {
        let candidate = estimate_messages_tokens(std::slice::from_ref(&messages[start - 1]));
        if used.saturating_add(candidate) > max_tokens {
            break;
        }
        used = used.saturating_add(candidate);
        start -= 1;
    }

    while start < messages.len() && !is_turn_start(&messages[start]) {
        start += 1;
    }
    start
}

/// Whether a message starts a real user turn.
///
/// Tool results are stored in `Role::User` messages, so those are not turn
/// boundaries: keeping them without their originating assistant `ToolUse` would
/// produce an orphan tool result.
fn is_turn_start(message: &Message) -> bool {
    if message.role != Role::User {
        return false;
    }
    for block in &message.content {
        if matches!(block, ContentBlock::ToolResult { .. }) {
            return false;
        }
    }
    true
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Appended to the summary prompt when the oldest messages had to be dropped to
/// fit the request, so the summary states what it could not see.
const TRUNCATED_NOTICE: &str = concat!(
    "\n\nNOTE: the earliest messages were dropped to fit the model context window. ",
    "Summarize what remains and say that earlier turns were omitted.",
);

/// Room reserved for the summary prompt text itself (tokens).
const COMPACT_PROMPT_RESERVE: u64 = 1_500;

/// Trim the oldest messages until the summary request fits the model window.
///
/// Returns the (possibly shortened) conversation plus whether anything was
/// dropped. The budget is `context_window - fixed completion budget - prompt
/// reserve`; the completion budget is not configurable in the engine, so it has
/// to be subtracted here rather than left to the provider.
fn fit_to_window(mut messages: Vec<Message>, context_window: usize) -> (Vec<Message>, bool) {
    let budget = (context_window as u64)
        .saturating_sub(COMPACT_MAX_OUTPUT_TOKENS as u64)
        .saturating_sub(COMPACT_PROMPT_RESERVE);
    let mut trimmed = false;
    // `truncate_for_retry` always drops at least one message, so the loop
    // terminates; the counter only guards against a future change that stops
    // shrinking.
    for _ in 0..256 {
        if estimate_messages_tokens(&messages) <= budget {
            break;
        }
        match truncate_for_retry(&messages) {
            Some(next) => {
                messages = next;
                trimmed = true;
            }
            // A single message that already exceeds the budget: send it and let
            // the provider decide rather than dropping the whole conversation.
            None => break,
        }
    }
    (messages, trimmed)
}

/// Collect all text from a streaming LLM response.
async fn collect_stream_text(mut rx: mpsc::Receiver<LlmEvent>) -> Result<(String, TokenUsage), CompactError> {
    let mut text = String::new();

    while let Some(event) = rx.recv().await {
        match event {
            LlmEvent::TextDelta(delta) => text.push_str(&delta),
            LlmEvent::Done { usage, .. } => return Ok((text, usage)),
            LlmEvent::Error(e) => return Err(CompactError::StreamError(e)),
            // Ignore thinking deltas and tool calls (shouldn't happen in compact)
            _ => {}
        }
    }

    // Channel closed without a Done event
    Err(CompactError::EmptyResponse)
}

/// Message prepended when the oldest messages are dropped, so the model knows
/// earlier turns were omitted.
const TRUNCATE_PLACEHOLDER: &str = "[earlier conversation truncated for compaction retry]";

fn is_truncate_placeholder(message: &Message) -> bool {
    if message.role != Role::User {
        return false;
    }
    for block in &message.content {
        if let ContentBlock::Text { text } = block
            && text.as_str() == TRUNCATE_PLACEHOLDER
        {
            return true;
        }
    }
    false
}

/// Truncate the oldest ~20% of messages for PTL retry.
///
/// Returns `None` if there are too few messages to truncate meaningfully.
fn truncate_for_retry(messages: &[Message]) -> Option<Vec<Message>> {
    if messages.len() < 2 {
        return None;
    }

    // Keep any leading placeholder out of the droppable set. Otherwise an
    // automated trimmer (e.g. `fit_to_window`) that drops a single message per
    // pass can delete only the placeholder and re-insert it on the next pass,
    // looping forever without ever removing real content.
    let (prefix, rest) = match messages.first() {
        Some(first) if is_truncate_placeholder(first) => (Some(first), &messages[1..]),
        _ => (None, messages),
    };

    let drop_count = (rest.len() / 5).max(1);
    if drop_count >= rest.len() {
        return None;
    }

    let remaining = &rest[drop_count..];
    let mut result = Vec::with_capacity(remaining.len() + 1);

    // Ensure the first message is User role for API compatibility, reusing the
    // existing placeholder instead of stacking a second one.
    match prefix {
        Some(placeholder) => result.push(placeholder.clone()),
        None if remaining.first().map(|m| m.role) != Some(Role::User) => result.push(Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: TRUNCATE_PLACEHOLDER.to_string(),
            }],
        )),
        _ => {}
    }

    result.extend_from_slice(remaining);
    Some(result)
}

/// Check if a message is a compact boundary marker.
pub fn is_compact_boundary(message: &Message) -> bool {
    message.content.iter().any(|block| {
        if let ContentBlock::Text { text } = block {
            text.starts_with(BOUNDARY_PREFIX)
        } else {
            false
        }
    })
}

/// Extract [`CompactMetadata`] from a boundary marker message.
pub fn extract_compact_metadata(message: &Message) -> Option<CompactMetadata> {
    for block in &message.content {
        if let ContentBlock::Text { text } = block
            && let Some(json_str) = text.strip_prefix(BOUNDARY_PREFIX)
        {
            let json_str = json_str.trim_start_matches('\n');
            return serde_json::from_str(json_str).ok();
        }
    }
    None
}

#[cfg(test)]
#[path = "auto_test.rs"]
mod auto_test;
