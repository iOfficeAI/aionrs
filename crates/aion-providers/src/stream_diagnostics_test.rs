use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aion_types::llm::LlmEvent;
use aion_types::message::{StopReason, TokenUsage};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::json;
use tracing::{Level, subscriber};
use tracing_subscriber::fmt::MakeWriter;

use super::*;

#[derive(Clone, Default)]
struct SharedLogWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

struct SharedLogGuard {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl Write for SharedLogGuard {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes
            .lock()
            .expect("log buffer mutex should not be poisoned")
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for SharedLogWriter {
    type Writer = SharedLogGuard;

    fn make_writer(&'writer self) -> Self::Writer {
        SharedLogGuard {
            bytes: Arc::clone(&self.bytes),
        }
    }
}

impl SharedLogWriter {
    fn contents(&self) -> String {
        String::from_utf8(
            self.bytes
                .lock()
                .expect("log buffer mutex should not be poisoned")
                .clone(),
        )
        .expect("structured log should be UTF-8")
    }
}

fn data_frame() -> Frame {
    Frame {
        event: None,
        data: String::new(),
        kind: FrameKind::Data,
    }
}

fn done_frame() -> Frame {
    Frame {
        event: None,
        data: String::new(),
        kind: FrameKind::Done,
    }
}

#[test]
fn diagnostics_retain_shape_without_payload_values() {
    let mut diagnostics = OpenAiStreamDiagnostics::default();
    let secret_content = "SENSITIVE_VISIBLE_CONTENT";
    let secret_reasoning = "SENSITIVE_REASONING_CONTENT";
    let payload = json!({
        "choices": [{
            "delta": {
                "content": secret_content,
                "reasoning_content": secret_reasoning,
                "reasoning": "alternative reasoning",
                "tool_calls": []
            },
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 3}
    });

    diagnostics.observe_frame(&data_frame());
    diagnostics.observe_json(&payload);
    diagnostics.observe_event(&LlmEvent::TextDelta(secret_content.to_string()));
    diagnostics.observe_event(&LlmEvent::ThinkingDelta(secret_reasoning.to_string()));
    diagnostics.observe_frame(&done_frame());
    diagnostics.observe_event(&LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        usage: TokenUsage::default(),
    });

    assert_eq!(diagnostics.content_delta_count, 1);
    assert_eq!(diagnostics.content_bytes, secret_content.len() as u64);
    assert_eq!(diagnostics.reasoning_delta_count, 1);
    assert_eq!(diagnostics.reasoning_bytes, secret_reasoning.len() as u64);
    assert!(diagnostics.delta_reasoning_field_seen);
    assert!(!diagnostics.is_anomalous(StreamTermination::Done));

    let debug = format!("{diagnostics:?}");
    assert!(!debug.contains(secret_content));
    assert!(!debug.contains(secret_reasoning));
    assert!(!debug.contains("alternative reasoning"));
}

#[test]
fn thinking_only_response_is_anomalous() {
    let mut diagnostics = OpenAiStreamDiagnostics::default();
    diagnostics.observe_frame(&data_frame());
    diagnostics.observe_json(&json!({
        "choices": [{
            "delta": {"reasoning_content": "private reasoning"},
            "finish_reason": "stop"
        }]
    }));
    diagnostics.observe_event(&LlmEvent::ThinkingDelta("private reasoning".to_string()));
    diagnostics.observe_frame(&done_frame());

    assert!(diagnostics.is_anomalous(StreamTermination::Done));
    assert_eq!(diagnostics.parsed_thinking_event_count, 1);
    assert_eq!(diagnostics.parsed_text_event_count, 0);
}

#[test]
fn alternative_response_shapes_are_recorded_as_presence_only() {
    let mut diagnostics = OpenAiStreamDiagnostics::default();
    diagnostics.observe_json(&json!({
        "type": "response.output_text.delta",
        "delta": "sensitive top-level delta",
        "output": [{"content": "sensitive output"}],
        "output_text": "sensitive output text",
        "choices": [{
            "message": {"content": "sensitive message"},
            "text": "sensitive legacy text",
            "delta": {
                "reasoning": "sensitive reasoning",
                "thinking": "sensitive thinking",
                "analysis": "sensitive analysis",
                "output_text": "sensitive delta output"
            },
            "finish_reason": "custom_reason"
        }]
    }));

    assert!(diagnostics.top_level_type_field_seen);
    assert!(diagnostics.top_level_delta_field_seen);
    assert!(diagnostics.top_level_output_field_seen);
    assert!(diagnostics.top_level_output_text_field_seen);
    assert!(diagnostics.choice_message_field_seen);
    assert!(diagnostics.choice_message_content_field_seen);
    assert!(diagnostics.choice_text_field_seen);
    assert!(diagnostics.delta_reasoning_field_seen);
    assert!(diagnostics.delta_thinking_field_seen);
    assert!(diagnostics.delta_analysis_field_seen);
    assert!(diagnostics.delta_output_text_field_seen);
    assert_eq!(diagnostics.finish_reason, FinishReasonKind::Other);

    let debug = format!("{diagnostics:?}");
    assert!(!debug.contains("sensitive"));
}

#[test]
fn request_id_is_sanitized_and_bounded() {
    let mut headers = HeaderMap::new();
    let request_id = "r".repeat(MAX_REQUEST_ID_CHARS + 20);
    headers.insert(
        "x-request-id",
        HeaderValue::from_str(&request_id).expect("request ID should be a valid header"),
    );
    let mut diagnostics = OpenAiStreamDiagnostics::default();

    diagnostics.observe_response(200, &headers);

    assert_eq!(diagnostics.http_status, 200);
    assert_eq!(
        diagnostics.request_id.as_deref().map(str::len),
        Some(MAX_REQUEST_ID_CHARS)
    );
}

#[test]
fn invalid_json_or_incomplete_stream_is_anomalous() {
    let mut diagnostics = OpenAiStreamDiagnostics::default();
    diagnostics.observe_invalid_json();

    assert!(diagnostics.is_anomalous(StreamTermination::Eof));
    assert_eq!(diagnostics.invalid_json_frame_count, 1);
}

#[test]
fn unsupported_standard_finish_reason_is_classified_without_wire_value_storage() {
    let mut diagnostics = OpenAiStreamDiagnostics::default();
    diagnostics.observe_json(&json!({
        "choices": [{
            "delta": {"content": "blocked"},
            "finish_reason": "content_filter"
        }]
    }));
    diagnostics.observe_event(&LlmEvent::TextDelta("blocked".to_string()));
    diagnostics.observe_frame(&done_frame());

    assert_eq!(diagnostics.finish_reason, FinishReasonKind::ContentFilter);
    assert!(diagnostics.is_anomalous(StreamTermination::Done));
}

#[test]
fn emitted_summary_contains_shape_but_not_payload_values() {
    let secret_content = "SENSITIVE_LOG_CONTENT";
    let secret_reasoning = "SENSITIVE_LOG_REASONING";
    let mut diagnostics = OpenAiStreamDiagnostics::default();
    diagnostics.observe_json(&json!({
        "choices": [{
            "delta": {
                "content": secret_content,
                "reasoning_content": secret_reasoning
            },
            "finish_reason": "stop"
        }]
    }));
    diagnostics.observe_event(&LlmEvent::TextDelta(secret_content.to_string()));
    diagnostics.observe_event(&LlmEvent::ThinkingDelta(secret_reasoning.to_string()));
    diagnostics.observe_frame(&done_frame());

    let writer = SharedLogWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(Level::TRACE)
        .with_writer(writer.clone())
        .finish();
    subscriber::with_default(subscriber, || {
        diagnostics.emit_summary(StreamTermination::Done, Duration::from_millis(42), 10, 3);
    });

    let output = writer.contents();
    assert!(output.contains("provider_stream_summary"));
    assert!(output.contains("\"content_delta_count\":1"));
    assert!(output.contains("\"reasoning_delta_count\":1"));
    assert!(output.contains("\"duration_ms\":42"));
    assert!(!output.contains(secret_content));
    assert!(!output.contains(secret_reasoning));
}
