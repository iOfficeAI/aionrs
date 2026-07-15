//! Setup and top-level orchestration for the JSON stream protocol.
//!
//! Flow: build protocol plumbing (writer/sink/approval manager) → bootstrap
//! the engine (silently resuming a session if requested — unlike the
//! terminal REPL, JSON stream mode never prints a resume banner) → emit
//! `Ready` → drain the `AddMcpServer` pre-message phase → run the main
//! command loop, dispatching `Message` to `message::handle` and everything
//! else to `dispatch::handle` → shut down all MCP managers on exit.

use std::sync::Arc;

use aion_agent::output::OutputSink;
use aion_agent::output::protocol_sink::ProtocolSink;
use aion_config::config::Config;
use aion_protocol::ToolApprovalManager;
use aion_protocol::commands::ProtocolCommand;
use aion_protocol::reader::spawn_stdin_reader;
use aion_protocol::writer::ProtocolWriter;

use super::context::StreamContext;
use super::dispatch::DispatchOutcome;
use super::pre_message::PreMessageOutcome;
use super::{dispatch, message, pre_message};
use crate::bootstrap::build_engine;

const ATTACHED_FILES_HEADER: &str = "[Attached files]";

pub(crate) async fn run(
    config: Config,
    cwd: &str,
    resume: Option<String>,
    session_id: Option<String>,
) -> anyhow::Result<()> {
    let writer = Arc::new(ProtocolWriter::new());
    let protocol_sink = Arc::new(ProtocolSink::new(writer.clone()));
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let output: Arc<dyn OutputSink> = protocol_sink.clone();

    let provider_name = config.provider_label.clone();

    // JSON stream mode never prints a resume banner — the host is expected
    // to render its own resume UX from the `Ready` event's session_id.
    let result = build_engine(config, cwd, output.clone(), resume.as_deref(), |_session| {}).await?;
    let mut engine = result.engine;
    let initial_has_mcp = result.has_mcp;

    if resume.is_none() {
        engine.init_session(&provider_name, cwd, session_id.as_deref())?;
    }

    let sid = engine.current_session_id();
    protocol_sink.emit_ready(engine.compat(), initial_has_mcp, sid, &approval_manager.current_mode());

    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer.clone());

    let mut cmd_rx = spawn_stdin_reader();

    let (dynamic_managers, mut pending_cmd) = match pre_message::run(&mut cmd_rx, &mut engine, &output, &writer).await {
        PreMessageOutcome::Stop => return Ok(()),
        PreMessageOutcome::Continue {
            dynamic_managers,
            next_command,
        } => (dynamic_managers, next_command.map(|c| *c)),
    };

    let has_mcp = initial_has_mcp || !dynamic_managers.is_empty();

    let ctx = StreamContext {
        output: output.clone(),
        writer: writer.clone(),
        approval_manager: approval_manager.clone(),
        protocol_sink: protocol_sink.clone(),
        has_mcp,
    };

    loop {
        let cmd = if let Some(c) = pending_cmd.take() {
            c
        } else {
            match cmd_rx.recv().await {
                Some(c) => c,
                None => break,
            }
        };

        if let ProtocolCommand::Message { msg_id, content, files } = cmd {
            let content = content_with_attachment_paths(&content, &files);
            let stopped = message::handle(&msg_id, &content, &mut engine, &mut cmd_rx, &ctx).await;
            if stopped {
                break;
            }
            continue;
        }

        match dispatch::handle(cmd, &mut engine, &ctx) {
            DispatchOutcome::Stop => break,
            DispatchOutcome::Continue => {}
        }
    }

    engine.run_stop_hooks().await;
    for mgr in &result.mcp_managers {
        mgr.shutdown().await;
    }
    for mgr in &dynamic_managers {
        mgr.shutdown().await;
    }

    Ok(())
}

fn content_with_attachment_paths(content: &str, files: &[String]) -> String {
    if files.is_empty() {
        return content.to_owned();
    }

    let mut input = content.trim().to_owned();
    if !input.is_empty() {
        input.push_str("\n\n");
    }
    input.push_str(ATTACHED_FILES_HEADER);
    for file in files {
        input.push('\n');
        input.push_str(file);
    }
    input
}

#[cfg(test)]
#[path = "session_test.rs"]
mod session_test;
