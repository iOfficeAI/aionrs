use std::time::Duration;

use aion_types::llm::LlmEvent;
use reqwest::header::HeaderMap;
use serde_json::Value;
use tracing::Level;

use crate::framing::{Frame, FrameKind};

const REQUEST_ID_HEADERS: &[&str] = &["x-request-id", "request-id"];
const MAX_REQUEST_ID_CHARS: usize = 128;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum FinishReasonKind {
    #[default]
    Missing,
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    FunctionCall,
    Other,
}

impl FinishReasonKind {
    fn from_wire(value: &str) -> Self {
        match value {
            "stop" => Self::Stop,
            "tool_calls" => Self::ToolCalls,
            "length" => Self::Length,
            "content_filter" => Self::ContentFilter,
            "function_call" => Self::FunctionCall,
            _ => Self::Other,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Stop => "stop",
            Self::ToolCalls => "tool_calls",
            Self::Length => "length",
            Self::ContentFilter => "content_filter",
            Self::FunctionCall => "function_call",
            Self::Other => "other",
        }
    }

    fn is_supported(self) -> bool {
        matches!(self, Self::Stop | Self::ToolCalls | Self::Length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StreamTermination {
    Done,
    Eof,
    ConsumerDropped,
    ConnectionError,
}

impl StreamTermination {
    fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Eof => "eof",
            Self::ConsumerDropped => "consumer_dropped",
            Self::ConnectionError => "connection_error",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct OpenAiStreamDiagnostics {
    // HTTP and framing.
    http_status: u16,
    request_id: Option<String>,
    network_chunk_count: u64,
    network_bytes: u64,
    sse_frame_count: u64,
    sse_data_frame_count: u64,
    done_seen: bool,

    // JSON envelope shape.
    valid_json_frame_count: u64,
    invalid_json_frame_count: u64,
    choices_field_seen: bool,
    choices_array_frame_count: u64,
    empty_choices_frame_count: u64,
    delta_field_seen: bool,
    usage_field_seen: bool,
    finish_reason_field_seen: bool,
    finish_reason: FinishReasonKind,

    // Recognized OpenAI-compatible output fields. Only presence and sizes are
    // retained; payload values are never stored.
    content_field_seen: bool,
    content_delta_count: u64,
    content_bytes: u64,
    content_non_whitespace_seen: bool,
    reasoning_content_field_seen: bool,
    reasoning_delta_count: u64,
    reasoning_bytes: u64,
    tool_calls_field_seen: bool,
    tool_call_delta_count: u64,
    tool_call_count: u64,

    // Common alternative response shapes that the current projector does not
    // consume. Record booleans only so compatibility gaps are diagnosable
    // without logging model output.
    delta_reasoning_field_seen: bool,
    delta_thinking_field_seen: bool,
    delta_analysis_field_seen: bool,
    delta_output_text_field_seen: bool,
    choice_message_field_seen: bool,
    choice_message_content_field_seen: bool,
    choice_text_field_seen: bool,
    top_level_output_field_seen: bool,
    top_level_output_text_field_seen: bool,
    top_level_type_field_seen: bool,
    top_level_delta_field_seen: bool,

    // Provider-neutral events produced by the parser.
    parsed_text_event_count: u64,
    parsed_thinking_event_count: u64,
    parsed_tool_call_event_count: u64,
    parsed_done_event_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StreamAssessment {
    empty_answer: bool,
    incomplete_stream: bool,
    malformed_json: bool,
    unexpected_finish_reason: bool,
}

impl StreamAssessment {
    fn is_anomalous(self) -> bool {
        self.empty_answer || self.incomplete_stream || self.malformed_json || self.unexpected_finish_reason
    }
}

impl OpenAiStreamDiagnostics {
    pub(crate) fn observe_response(&mut self, status: u16, headers: &HeaderMap) {
        self.http_status = status;
        self.request_id = REQUEST_ID_HEADERS.iter().find_map(|name| {
            let value = headers.get(*name)?.to_str().ok()?;
            let sanitized = value
                .chars()
                .filter(|character| !character.is_control())
                .take(MAX_REQUEST_ID_CHARS)
                .collect::<String>();
            (!sanitized.is_empty()).then_some(sanitized)
        });
    }

    pub(crate) fn observe_network_chunk(&mut self, byte_count: usize) {
        self.network_chunk_count = self.network_chunk_count.saturating_add(1);
        self.network_bytes = self.network_bytes.saturating_add(usize_to_u64(byte_count));
    }

    pub(crate) fn observe_frame(&mut self, frame: &Frame) {
        self.sse_frame_count = self.sse_frame_count.saturating_add(1);
        match frame.kind {
            FrameKind::Data => {
                self.sse_data_frame_count = self.sse_data_frame_count.saturating_add(1);
            }
            FrameKind::Done => self.done_seen = true,
        }
    }

    pub(crate) fn observe_invalid_json(&mut self) {
        self.invalid_json_frame_count = self.invalid_json_frame_count.saturating_add(1);
    }

    pub(crate) fn observe_json(&mut self, json: &Value) {
        self.valid_json_frame_count = self.valid_json_frame_count.saturating_add(1);
        self.usage_field_seen |= json.get("usage").is_some();
        self.top_level_output_field_seen |= json.get("output").is_some();
        self.top_level_output_text_field_seen |= json.get("output_text").is_some();
        self.top_level_type_field_seen |= json.get("type").is_some();
        self.top_level_delta_field_seen |= json.get("delta").is_some();

        let Some(choices) = json.get("choices") else {
            return;
        };
        self.choices_field_seen = true;

        let Some(choices) = choices.as_array() else {
            return;
        };
        self.choices_array_frame_count = self.choices_array_frame_count.saturating_add(1);
        if choices.is_empty() {
            self.empty_choices_frame_count = self.empty_choices_frame_count.saturating_add(1);
            return;
        }

        let choice = &choices[0];
        self.choice_message_field_seen |= choice.get("message").is_some();
        self.choice_message_content_field_seen |= choice
            .get("message")
            .and_then(|message| message.get("content"))
            .is_some();
        self.choice_text_field_seen |= choice.get("text").is_some();

        if let Some(finish_reason) = choice.get("finish_reason") {
            self.finish_reason_field_seen = true;
            if let Some(finish_reason) = finish_reason.as_str() {
                self.finish_reason = FinishReasonKind::from_wire(finish_reason);
            }
        }

        let Some(delta) = choice.get("delta") else {
            return;
        };
        self.delta_field_seen = true;
        self.delta_reasoning_field_seen |= delta.get("reasoning").is_some();
        self.delta_thinking_field_seen |= delta.get("thinking").is_some();
        self.delta_analysis_field_seen |= delta.get("analysis").is_some();
        self.delta_output_text_field_seen |= delta.get("output_text").is_some();

        if let Some(content) = delta.get("content") {
            self.content_field_seen = true;
            if let Some(content) = content.as_str().filter(|content| !content.is_empty()) {
                self.content_delta_count = self.content_delta_count.saturating_add(1);
                self.content_bytes = self.content_bytes.saturating_add(usize_to_u64(content.len()));
                self.content_non_whitespace_seen |= content.chars().any(|character| !character.is_whitespace());
            }
        }

        if let Some(reasoning) = delta.get("reasoning_content") {
            self.reasoning_content_field_seen = true;
            if let Some(reasoning) = reasoning.as_str().filter(|reasoning| !reasoning.is_empty()) {
                self.reasoning_delta_count = self.reasoning_delta_count.saturating_add(1);
                self.reasoning_bytes = self.reasoning_bytes.saturating_add(usize_to_u64(reasoning.len()));
            }
        }

        if let Some(tool_calls) = delta.get("tool_calls") {
            self.tool_calls_field_seen = true;
            self.tool_call_delta_count = self.tool_call_delta_count.saturating_add(1);
            if let Some(tool_calls) = tool_calls.as_array() {
                self.tool_call_count = self.tool_call_count.saturating_add(usize_to_u64(tool_calls.len()));
            }
        }
    }

    pub(crate) fn observe_event(&mut self, event: &LlmEvent) {
        match event {
            LlmEvent::TextDelta(_) => {
                self.parsed_text_event_count = self.parsed_text_event_count.saturating_add(1);
            }
            LlmEvent::ThinkingDelta(_) => {
                self.parsed_thinking_event_count = self.parsed_thinking_event_count.saturating_add(1);
            }
            LlmEvent::ToolUse { .. } => {
                self.parsed_tool_call_event_count = self.parsed_tool_call_event_count.saturating_add(1);
            }
            LlmEvent::Done { .. } => {
                self.parsed_done_event_count = self.parsed_done_event_count.saturating_add(1);
            }
            LlmEvent::ThinkingSignature(_) | LlmEvent::Error(_) => {}
        }
    }

    pub(crate) fn emit_summary(
        &self,
        termination: StreamTermination,
        duration: Duration,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        let duration_ms = u128_to_u64(duration.as_millis());
        let request_id = self.request_id.as_deref().unwrap_or("none");
        let assessment = self.assess(termination);

        macro_rules! emit {
            ($level:expr) => {
                tracing::event!(
                    target: "aion_providers",
                    $level,
                    diagnostic_event = "provider_stream_summary",
                    protocol = "openai_sse",
                    http_status = self.http_status,
                    request_id,
                    duration_ms,
                    termination = termination.as_str(),
                    network_chunk_count = self.network_chunk_count,
                    network_bytes = self.network_bytes,
                    sse_frame_count = self.sse_frame_count,
                    sse_data_frame_count = self.sse_data_frame_count,
                    done_seen = self.done_seen,
                    valid_json_frame_count = self.valid_json_frame_count,
                    invalid_json_frame_count = self.invalid_json_frame_count,
                    choices_field_seen = self.choices_field_seen,
                    choices_array_frame_count = self.choices_array_frame_count,
                    empty_choices_frame_count = self.empty_choices_frame_count,
                    delta_field_seen = self.delta_field_seen,
                    usage_field_seen = self.usage_field_seen,
                    input_tokens,
                    output_tokens,
                    finish_reason_field_seen = self.finish_reason_field_seen,
                    finish_reason = self.finish_reason.as_str(),
                    content_field_seen = self.content_field_seen,
                    content_delta_count = self.content_delta_count,
                    content_bytes = self.content_bytes,
                    content_non_whitespace_seen = self.content_non_whitespace_seen,
                    reasoning_content_field_seen = self.reasoning_content_field_seen,
                    reasoning_delta_count = self.reasoning_delta_count,
                    reasoning_bytes = self.reasoning_bytes,
                    tool_calls_field_seen = self.tool_calls_field_seen,
                    tool_call_delta_count = self.tool_call_delta_count,
                    tool_call_count = self.tool_call_count,
                    delta_reasoning_field_seen = self.delta_reasoning_field_seen,
                    delta_thinking_field_seen = self.delta_thinking_field_seen,
                    delta_analysis_field_seen = self.delta_analysis_field_seen,
                    delta_output_text_field_seen = self.delta_output_text_field_seen,
                    choice_message_field_seen = self.choice_message_field_seen,
                    choice_message_content_field_seen = self.choice_message_content_field_seen,
                    choice_text_field_seen = self.choice_text_field_seen,
                    top_level_output_field_seen = self.top_level_output_field_seen,
                    top_level_output_text_field_seen = self.top_level_output_text_field_seen,
                    top_level_type_field_seen = self.top_level_type_field_seen,
                    top_level_delta_field_seen = self.top_level_delta_field_seen,
                    parsed_text_event_count = self.parsed_text_event_count,
                    parsed_thinking_event_count = self.parsed_thinking_event_count,
                    parsed_tool_call_event_count = self.parsed_tool_call_event_count,
                    parsed_done_event_count = self.parsed_done_event_count,
                    empty_answer = assessment.empty_answer,
                    incomplete_stream = assessment.incomplete_stream,
                    malformed_json = assessment.malformed_json,
                    unexpected_finish_reason = assessment.unexpected_finish_reason,
                    "provider stream response shape"
                );
            };
        }

        if assessment.is_anomalous() {
            emit!(Level::WARN);
        } else {
            emit!(Level::DEBUG);
        }
    }

    fn assess(&self, termination: StreamTermination) -> StreamAssessment {
        if termination == StreamTermination::ConsumerDropped {
            return StreamAssessment {
                empty_answer: false,
                incomplete_stream: false,
                malformed_json: false,
                unexpected_finish_reason: false,
            };
        }

        StreamAssessment {
            empty_answer: !self.content_non_whitespace_seen && self.parsed_tool_call_event_count == 0,
            incomplete_stream: termination != StreamTermination::Done || !self.done_seen,
            malformed_json: self.invalid_json_frame_count > 0,
            unexpected_finish_reason: !self.finish_reason.is_supported(),
        }
    }
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn u128_to_u64(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "stream_diagnostics_test.rs"]
mod stream_diagnostics_test;
