use aion_config::compact::CompactConfig;
use std::sync::Arc;

use aion_config::compat::ProviderCompat;
use aion_protocol::events::{Capabilities, CompactionInfo, ErrorInfo, ProtocolEvent, Usage};
use aion_protocol::writer::{ProtocolEmitter, ProtocolWriter};
use aion_types::llm::ProviderMetadata;

use super::OutputSink;

/// JSON stream protocol output sink
pub struct ProtocolSink {
    writer: Arc<ProtocolWriter>,
}

pub struct CapabilitySnapshot<'a> {
    pub compat: &'a ProviderCompat,
    pub has_mcp: bool,
    pub current_mode: &'a str,
    pub current_model: &'a str,
    pub provider_metadata: &'a ProviderMetadata,
    pub compact: &'a CompactConfig,
}

impl ProtocolSink {
    pub fn new(writer: Arc<ProtocolWriter>) -> Self {
        Self { writer }
    }

    /// Emit the ready event at session start
    pub fn emit_ready(&self, session_id: Option<String>, snapshot: &CapabilitySnapshot<'_>) {
        let _ = self.writer.emit(&ProtocolEvent::Ready {
            version: env!("CARGO_PKG_VERSION").to_string(),
            session_id,
            capabilities: Self::build_capabilities(snapshot),
        });
    }

    /// Emit a config_changed event after set_config or set_mode updates
    pub fn emit_config_changed(&self, snapshot: &CapabilitySnapshot<'_>) {
        let _ = self.writer.emit(&ProtocolEvent::ConfigChanged {
            capabilities: Self::build_capabilities(snapshot),
        });
    }

    /// Access the underlying writer for custom events
    pub fn writer(&self) -> &Arc<ProtocolWriter> {
        &self.writer
    }

    fn build_capabilities(snapshot: &CapabilitySnapshot<'_>) -> Capabilities {
        let context_limit = snapshot
            .provider_metadata
            .models
            .iter()
            .find(|model| model.id == snapshot.current_model)
            .and_then(|model| model.context_window)
            .map(|limit| limit.min(snapshot.compact.context_window as u64))
            .or(Some(snapshot.compact.context_window as u64));

        Capabilities {
            tool_approval: true,
            thinking: snapshot.compat.supports_thinking(),
            effort: snapshot.compat.supports_effort(),
            effort_levels: snapshot.compat.effort_levels().to_vec(),
            modes: vec!["default".into(), "auto_edit".into(), "yolo".into()],
            current_mode: snapshot.current_mode.to_string(),
            mcp: snapshot.has_mcp,
            current_model: Some(snapshot.current_model.to_string()),
            available_models: snapshot.provider_metadata.models.clone(),
            account_limits: snapshot.provider_metadata.account_limits.clone(),
            context_limit,
            compaction: Some(CompactionInfo {
                enabled: snapshot.compact.enabled,
                context_window: snapshot.compact.context_window as u64,
                output_reserve: snapshot.compact.output_reserve as u64,
                autocompact_trigger: snapshot
                    .compact
                    .context_window
                    .saturating_sub(snapshot.compact.output_reserve)
                    .saturating_sub(snapshot.compact.autocompact_buffer)
                    as u64,
                emergency_limit: snapshot
                    .compact
                    .context_window
                    .saturating_sub(snapshot.compact.emergency_buffer)
                    as u64,
            }),
        }
    }
}

impl OutputSink for ProtocolSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        let _ = self.writer.emit(&ProtocolEvent::TextDelta {
            text: text.to_string(),
            msg_id: msg_id.to_string(),
        });
    }

    fn emit_thinking(&self, text: &str, msg_id: &str) {
        let _ = self.writer.emit(&ProtocolEvent::Thinking {
            text: text.to_string(),
            msg_id: msg_id.to_string(),
        });
    }

    fn emit_tool_call(&self, name: &str, _input: &str) {
        // In protocol mode, tool_call is handled by tool_request/tool_running events.
        // This is a fallback for compatibility.
        let _ = self.writer.emit(&ProtocolEvent::Info {
            msg_id: String::new(),
            message: format!("Tool call: {name}"),
        });
    }

    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        // In protocol mode, tool results are emitted via explicit ToolResult events
        // with call_id. This fallback emits an info event.
        let status = if is_error { "error" } else { "success" };
        let _ = self.writer.emit(&ProtocolEvent::Info {
            msg_id: String::new(),
            message: format!("[{name} {status}] {content}"),
        });
    }

    fn emit_stream_start(&self, msg_id: &str) {
        let _ = self.writer.emit(&ProtocolEvent::StreamStart {
            msg_id: msg_id.to_string(),
        });
    }

    fn emit_stream_end(
        &self,
        msg_id: &str,
        _turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::StreamEnd {
            msg_id: msg_id.to_string(),
            usage: Some(Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens: if cache_read_tokens > 0 {
                    Some(cache_read_tokens)
                } else {
                    None
                },
                cache_write_tokens: if cache_creation_tokens > 0 {
                    Some(cache_creation_tokens)
                } else {
                    None
                },
            }),
        });
    }

    fn emit_error(&self, msg: &str) {
        let _ = self.writer.emit(&ProtocolEvent::Error {
            msg_id: None,
            error: ErrorInfo {
                code: "engine_error".to_string(),
                message: msg.to_string(),
                retryable: false,
            },
        });
    }

    fn emit_info(&self, msg: &str) {
        let _ = self.writer.emit(&ProtocolEvent::Info {
            msg_id: String::new(),
            message: msg.to_string(),
        });
    }

    fn emit_provider_retry(
        &self,
        msg_id: &str,
        attempt: u32,
        max_retries: u32,
        delay_ms: u64,
        error: &str,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::ProviderRetry {
            msg_id: msg_id.to_string(),
            attempt,
            max_retries,
            delay_ms,
            error: error.to_string(),
        });
    }
}
