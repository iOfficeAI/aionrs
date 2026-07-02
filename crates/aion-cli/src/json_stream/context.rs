//! Shared protocol I/O context threaded through the JSON stream main loop.
//!
//! `dispatch::handle` and `message::handle` both need the same cluster of
//! protocol plumbing (output sink, writer, approval manager, protocol sink,
//! and the current MCP availability flag). Grouping them here avoids passing
//! the same five arguments individually to every handler.

use std::sync::Arc;

use aion_agent::output::OutputSink;
use aion_agent::output::protocol_sink::ProtocolSink;
use aion_protocol::ToolApprovalManager;
use aion_protocol::writer::ProtocolWriter;

/// Protocol I/O handles shared across top-level command dispatch and
/// in-flight message handling.
///
/// `engine` and `cmd_rx` are deliberately excluded: they need independent
/// `&mut` borrows that would conflict with a shared `&StreamContext`.
pub(super) struct StreamContext {
    pub(super) output: Arc<dyn OutputSink>,
    pub(super) writer: Arc<ProtocolWriter>,
    pub(super) approval_manager: Arc<ToolApprovalManager>,
    pub(super) protocol_sink: Arc<ProtocolSink>,
    pub(super) has_mcp: bool,
}
