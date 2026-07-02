//! Top-level `ProtocolCommand` dispatch for the JSON stream main loop.
//!
//! Handles every command except `Message` (which needs the inner
//! select-loop machinery in `message.rs`) and the `AddMcpServer` pre-message
//! phase (handled in `pre_message.rs` before this loop starts).

use aion_agent::engine::AgentEngine;
use aion_protocol::ToolApprovalResult;
use aion_protocol::commands::ProtocolCommand;
use aion_protocol::events::ProtocolEvent;
use aion_protocol::writer::ProtocolEmitter;

use super::context::StreamContext;

/// Outcome of handling one top-level command.
pub(super) enum DispatchOutcome {
    /// Keep looping.
    Continue,
    /// A `Stop` command was received — shut down.
    Stop,
}

/// Handle a single top-level command (i.e. one that arrived outside of an
/// in-flight `Message`). `Message` itself is handled by the caller via
/// `message::handle`, not here.
pub(super) fn handle(cmd: ProtocolCommand, engine: &mut AgentEngine, ctx: &StreamContext) -> DispatchOutcome {
    match cmd {
        ProtocolCommand::Stop => return DispatchOutcome::Stop,
        ProtocolCommand::ToolApprove { call_id, scope: _ } => {
            ctx.approval_manager.resolve(&call_id, ToolApprovalResult::Approved);
        }
        ProtocolCommand::ToolDeny { call_id, reason } => {
            ctx.approval_manager
                .resolve(&call_id, ToolApprovalResult::Denied { reason });
        }
        ProtocolCommand::InitHistory { text } => {
            tracing::debug!(target: "aion_protocol", chars = text.len(), "InitHistory received");
        }
        ProtocolCommand::SetMode { mode } => {
            let mode_str = format!("{mode:?}").to_lowercase();
            ctx.approval_manager.set_mode(mode);
            let _ = ctx.writer.emit(&ProtocolEvent::Info {
                msg_id: String::new(),
                message: format!("mode updated: {}", ctx.approval_manager.current_mode()),
            });
            ctx.protocol_sink
                .emit_config_changed(engine.compat(), ctx.has_mcp, &ctx.approval_manager.current_mode());
            tracing::debug!(target: "aion_protocol", mode = %mode_str, "SetMode applied");
        }
        ProtocolCommand::SetConfig {
            model,
            thinking,
            thinking_budget,
            effort,
            compaction,
        } => {
            let changes = engine.apply_config_update(model, thinking, thinking_budget, effort, compaction);
            let message = if changes.is_empty() {
                "set_config: no changes".to_string()
            } else {
                format!("config updated: {}", changes.join(", "))
            };
            let _ = ctx.writer.emit(&ProtocolEvent::Info {
                msg_id: String::new(),
                message,
            });
            ctx.protocol_sink
                .emit_config_changed(engine.compat(), ctx.has_mcp, &ctx.approval_manager.current_mode());
        }
        ProtocolCommand::AddMcpServer { name, .. } => {
            ctx.output.emit_error(&format!(
                "AddMcpServer '{name}': rejected — only allowed before first Message"
            ));
        }
        ProtocolCommand::Ping => {
            let _ = ctx.writer.emit(&ProtocolEvent::Pong);
        }
        ProtocolCommand::Message { .. } => {
            // `Message` is routed to `message::handle` by the caller before
            // reaching this dispatcher. Reaching here means the caller's
            // routing changed; log and ignore rather than panic.
            tracing::warn!(
                target: "aion_protocol",
                "Message reached dispatch::handle; expected routing to message::handle"
            );
        }
    }

    DispatchOutcome::Continue
}
