//! Handling of a single `Message` command in the JSON stream protocol.
//!
//! While the engine is running a message, the host may still send control
//! commands (tool approve/deny, mode/config updates, stop, ping) over the
//! same stdin channel. This module races the engine's run future against
//! that command stream and applies queued config/mode changes once the
//! response finishes.

use aion_agent::engine::AgentEngine;
use aion_protocol::ToolApprovalResult;
use aion_protocol::commands::ProtocolCommand;
use aion_protocol::events::ProtocolEvent;
use aion_protocol::writer::ProtocolEmitter;
use tokio::sync::mpsc::UnboundedReceiver;

use super::context::StreamContext;

/// Pending config fields queued via `SetConfig` while a message is in
/// flight: (model, thinking, effort, compaction).
type PendingConfig = (Option<String>, Option<String>, Option<String>, Option<String>);

/// Run the engine on `content`, racing it against control commands on
/// `cmd_rx`. Returns `true` if a `Stop` command was received while the
/// message was in flight (caller should end the session).
pub(super) async fn handle(
    msg_id: &str,
    content: &str,
    engine: &mut AgentEngine,
    cmd_rx: &mut UnboundedReceiver<ProtocolCommand>,
    ctx: &StreamContext,
) -> bool {
    let mut stopped = false;
    let mut pending_config: Option<PendingConfig> = None;
    let mut mode_changed = false;

    {
        let engine_fut = engine.run(content, msg_id);
        tokio::pin!(engine_fut);

        loop {
            tokio::select! {
                result = &mut engine_fut => {
                    match result {
                        Ok(result) => {
                            ctx.output.emit_stream_end(
                                msg_id,
                                result.turns,
                                result.usage.input_tokens,
                                result.usage.output_tokens,
                                result.usage.cache_creation_tokens,
                                result.usage.cache_read_tokens,
                            );
                        }
                        Err(e) => {
                            ctx.output.emit_error(&e.to_string());
                            ctx.output.emit_stream_end(msg_id, 0, 0, 0, 0, 0);
                        }
                    }
                    break;
                }
                Some(sub_cmd) = cmd_rx.recv() => {
                    match sub_cmd {
                        ProtocolCommand::ToolApprove { call_id, scope: _ } => {
                            ctx.approval_manager.resolve(&call_id, ToolApprovalResult::Approved);
                        }
                        ProtocolCommand::ToolDeny { call_id, reason } => {
                            ctx.approval_manager.resolve(&call_id, ToolApprovalResult::Denied { reason });
                        }
                        ProtocolCommand::Stop => {
                            stopped = true;
                            break;
                        }
                        ProtocolCommand::SetConfig { model, thinking, effort, compaction } => {
                            pending_config = Some((model, thinking, effort, compaction));
                            let _ = ctx.writer.emit(&ProtocolEvent::Info {
                                msg_id: String::new(),
                                message: "set_config: queued, will apply after current response".to_string(),
                            });
                        }
                        ProtocolCommand::SetMode { mode } => {
                            ctx.approval_manager.set_mode(mode);
                            mode_changed = true;
                            let _ = ctx.writer.emit(&ProtocolEvent::Info {
                                msg_id: String::new(),
                                message: format!("mode updated: {}", ctx.approval_manager.current_mode()),
                            });
                        }
                        ProtocolCommand::Ping => {
                            let _ = ctx.writer.emit(&ProtocolEvent::Pong);
                        }
                        _ => {
                            tracing::debug!(target: "aion_protocol", "ignoring command during active message processing");
                        }
                    }
                }
            }
        }
    }

    if let Some((model, thinking, effort, compaction)) = pending_config.take() {
        let changes = engine.apply_config_update(model, thinking, effort, compaction);
        if !changes.is_empty() {
            let _ = ctx.writer.emit(&ProtocolEvent::Info {
                msg_id: String::new(),
                message: format!("config applied: {}", changes.join(", ")),
            });
        }
        ctx.protocol_sink
            .emit_config_changed(engine.compat(), ctx.has_mcp, &ctx.approval_manager.current_mode());
    } else if mode_changed {
        ctx.protocol_sink
            .emit_config_changed(engine.compat(), ctx.has_mcp, &ctx.approval_manager.current_mode());
    }

    stopped
}
