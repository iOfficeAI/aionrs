use std::time::Instant;

use tokio::sync::mpsc;

use aion_types::llm::LlmEvent;
use aion_types::message::{StopReason, TokenUsage};

use crate::error::ProviderError;
use crate::framing::{FrameKind, SseBlockFramer, SseLineFramer, bedrock_payload_to_frame};
use crate::parser::{AnthropicParser, OpenAiParser, ResponseParser};
use crate::stream_diagnostics::StreamTermination;
use crate::stream_runner::StreamOutcome;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StreamDecoder {
    OpenAiSseLine { auto_tool_id: bool },
    AnthropicSseBlock,
    BedrockAwsEventStream,
}

impl StreamDecoder {
    pub(crate) async fn process(self, response: reqwest::Response, tx: &mpsc::Sender<LlmEvent>) -> StreamOutcome {
        match self {
            Self::OpenAiSseLine { auto_tool_id } => process_openai_sse_stream(response, tx, auto_tool_id).await,
            Self::AnthropicSseBlock => process_anthropic_sse_stream(response, tx).await,
            Self::BedrockAwsEventStream => process_bedrock_aws_event_stream(response, tx).await,
        }
    }
}

pub(crate) async fn process_openai_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
    auto_tool_id: bool,
) -> StreamOutcome {
    use futures::StreamExt;

    let parser = OpenAiParser { auto_tool_id };
    let mut state = parser.new_state();
    state
        .diagnostics_mut()
        .observe_response(response.status().as_u16(), response.headers());
    let started_at = Instant::now();
    let mut framer = SseLineFramer::default();
    let mut stream = response.bytes_stream();
    let mut emitted_content = false;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let err = ProviderError::Connection(e.to_string());
                let outcome = if emitted_content {
                    StreamOutcome::FailedPartial(err)
                } else {
                    StreamOutcome::FailedEmpty(err)
                };
                state.emit_diagnostics(StreamTermination::ConnectionError, started_at.elapsed());
                return outcome;
            }
        };
        state.diagnostics_mut().observe_network_chunk(chunk.len());
        let text = String::from_utf8_lossy(&chunk);
        for frame in framer.push_text(&text, "[DONE]") {
            state.diagnostics_mut().observe_frame(&frame);
            let is_done = frame.kind == FrameKind::Done;
            let events = parser.parse_frame(&frame, &mut state);
            for event in events {
                state.diagnostics_mut().observe_event(&event);
                if matches!(
                    event,
                    LlmEvent::TextDelta(_) | LlmEvent::ThinkingDelta(_) | LlmEvent::ToolUse { .. }
                ) {
                    emitted_content = true;
                }
                if tx.send(event).await.is_err() {
                    state.emit_diagnostics(StreamTermination::ConsumerDropped, started_at.elapsed());
                    return StreamOutcome::Ok;
                }
            }
            if is_done {
                state.emit_diagnostics(StreamTermination::Done, started_at.elapsed());
                return StreamOutcome::Ok;
            }
        }
    }

    for event in parser.finish(&mut state) {
        state.diagnostics_mut().observe_event(&event);
        if tx.send(event).await.is_err() {
            state.emit_diagnostics(StreamTermination::ConsumerDropped, started_at.elapsed());
            return StreamOutcome::Ok;
        }
    }

    state.emit_diagnostics(StreamTermination::Eof, started_at.elapsed());
    StreamOutcome::Ok
}

pub(crate) async fn process_anthropic_sse_stream(
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
                    return StreamOutcome::Ok;
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

pub(crate) async fn process_bedrock_aws_event_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
) -> StreamOutcome {
    use futures::StreamExt;

    let parser = AnthropicParser;
    let mut state = parser.new_state();
    let mut buffer = Vec::new();
    let mut stream = response.bytes_stream();
    let mut emitted_content = false;
    let mut emitted_done = false;

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
        buffer.extend_from_slice(&chunk);

        while let Some((event_data, consumed)) = parse_aws_event(&buffer) {
            buffer = buffer[consumed..].to_vec();

            let Some(payload) = event_data else {
                continue;
            };

            if let Some(frame) = bedrock_payload_to_frame(&payload) {
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
                    if matches!(event, LlmEvent::Done { .. }) {
                        emitted_done = true;
                    }
                    if tx.send(event).await.is_err() {
                        return StreamOutcome::Ok;
                    }
                }
            }
        }
    }

    if !emitted_done && (state.input_tokens > 0 || state.output_tokens > 0) {
        let _ = tx
            .send(LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_creation_tokens: state.cache_creation_tokens,
                    cache_read_tokens: state.cache_read_tokens,
                },
            })
            .await;
    }

    StreamOutcome::Ok
}

/// Parse one AWS event stream message from the buffer.
/// Returns (Some(payload), bytes_consumed) if a complete message is found,
/// or None if more data is needed.
///
/// AWS event stream binary format:
/// - Prelude: total_len (4 bytes, big-endian) + headers_len (4 bytes) + prelude_crc (4 bytes)
/// - Headers: variable length
/// - Payload: variable length
/// - Message CRC: 4 bytes
fn parse_aws_event(buffer: &[u8]) -> Option<(Option<Vec<u8>>, usize)> {
    if buffer.len() < 12 {
        return None;
    }

    let total_len = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    let headers_len = u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]) as usize;

    if buffer.len() < total_len {
        return None;
    }

    let payload_start = 12 + headers_len;
    let payload_end = total_len - 4;

    if payload_start <= payload_end {
        let payload = buffer[payload_start..payload_end].to_vec();
        Some((Some(payload), total_len))
    } else {
        Some((None, total_len))
    }
}

#[cfg(test)]
#[path = "stream_process_test.rs"]
mod stream_process_test;
