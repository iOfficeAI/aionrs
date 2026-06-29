use tokio::sync::mpsc;

use aion_types::llm::LlmEvent;
use aion_types::message::{StopReason, TokenUsage};

use crate::error::ProviderError;
use crate::framing::{FrameKind, SseBlockFramer, SseLineFramer, bedrock_payload_to_frame};
use crate::parser::{AnthropicParser, OpenAiParser, ResponseParser};
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
                tracing::debug!(target: "aion_providers", chunk = %frame.data, "bedrock event chunk");
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
mod tests {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde_json::json;
    use tokio::sync::mpsc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn aws_event_message(payload: &[u8]) -> Vec<u8> {
        let total_len = 12 + payload.len() + 4;
        let mut message = Vec::with_capacity(total_len);
        message.extend_from_slice(&(total_len as u32).to_be_bytes());
        message.extend_from_slice(&0u32.to_be_bytes());
        message.extend_from_slice(&0u32.to_be_bytes());
        message.extend_from_slice(payload);
        message.extend_from_slice(&0u32.to_be_bytes());
        message
    }

    fn bedrock_event_payload(inner: &str) -> Vec<u8> {
        json!({
            "bytes": STANDARD.encode(inner)
        })
        .to_string()
        .into_bytes()
    }

    async fn mock_response(body: Vec<u8>) -> reqwest::Response {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        reqwest::get(format!("{}/stream", server.uri()))
            .await
            .expect("mock response should be available")
    }

    async fn collect_events(mut rx: mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    #[test]
    fn parse_aws_event_waits_for_complete_message_and_extracts_payload() {
        let payload = b"payload";
        let message = aws_event_message(payload);

        assert!(parse_aws_event(&message[..message.len() - 1]).is_none());

        let (event_data, consumed) = parse_aws_event(&message).expect("complete event should parse");
        assert_eq!(event_data, Some(payload.to_vec()));
        assert_eq!(consumed, message.len());
    }

    #[tokio::test]
    async fn bedrock_event_stream_decodes_payloads_into_llm_events() {
        let mut body = Vec::new();
        for inner in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":12}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
        ] {
            body.extend(aws_event_message(&bedrock_event_payload(inner)));
        }

        let response = mock_response(body).await;
        let (tx, rx) = mpsc::channel(8);

        let outcome = process_bedrock_aws_event_stream(response, &tx).await;
        drop(tx);
        let events = collect_events(rx).await;

        assert!(matches!(outcome, StreamOutcome::Ok));
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Hello"));
        match &events[1] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 12);
                assert_eq!(usage.output_tokens, 7);
            }
            event => panic!("expected Done event, got {event:?}"),
        }
    }

    #[tokio::test]
    async fn bedrock_event_stream_synthesizes_done_when_message_delta_is_missing() {
        let mut body = Vec::new();
        for inner in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":12}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        ] {
            body.extend(aws_event_message(&bedrock_event_payload(inner)));
        }

        let response = mock_response(body).await;
        let (tx, rx) = mpsc::channel(8);

        let outcome = process_bedrock_aws_event_stream(response, &tx).await;
        drop(tx);
        let events = collect_events(rx).await;

        assert!(matches!(outcome, StreamOutcome::Ok));
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Hello"));
        match &events[1] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 12);
                assert_eq!(usage.output_tokens, 0);
            }
            event => panic!("expected synthesized Done event, got {event:?}"),
        }
    }
}
