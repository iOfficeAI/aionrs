use std::sync::{Arc, Mutex};

use crate::confirm::{ConfirmResult, ToolConfirmer};
use crate::hooks::HookEngine;
use crate::protocol::{ToolApprovalManager, ToolApprovalResult};
use crate::protocol::events::{ToolCategory, ToolInfo, OutputType, ProtocolEvent, ToolStatus};
use crate::protocol::writer::ProtocolWriter;
use crate::types::message::ContentBlock;
use crate::types::tool::ToolResult;

use super::registry::ToolRegistry;

/// Partition tool calls and execute them with optional confirmation and hooks
pub async fn execute_tool_calls(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    hooks: Option<&HookEngine>,
) -> Result<Vec<ContentBlock>, ExecutionControl> {
    let mut results = Vec::new();

    for batch in partition(registry, tool_calls) {
        if batch.is_concurrent {
            // For concurrent batch, confirm all first, then execute approved ones
            let mut approved = Vec::new();
            for call in &batch.calls {
                match confirm_call(confirmer, call)? {
                    Some(denied) => results.push(denied),
                    None => approved.push(call),
                }
            }
            let futures: Vec<_> = approved
                .iter()
                .map(|call| execute_single(registry, call, hooks))
                .collect();
            let batch_results = futures::future::join_all(futures).await;
            results.extend(batch_results);
        } else {
            for call in &batch.calls {
                match confirm_call(confirmer, call)? {
                    Some(denied) => results.push(denied),
                    None => results.push(execute_single(registry, call, hooks).await),
                }
            }
        }
    }

    Ok(results)
}

/// Signal that the user wants to abort
#[derive(Debug)]
pub enum ExecutionControl {
    Quit,
}

/// Confirm a single tool call. Returns Ok(Some(result)) if denied, Ok(None) if approved, Err if quit.
fn confirm_call(
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    call: &ContentBlock,
) -> Result<Option<ContentBlock>, ExecutionControl> {
    let ContentBlock::ToolUse { id, name, input } = call else {
        return Ok(None);
    };

    let input_display = serde_json::to_string(input).unwrap_or_default();
    let result = confirmer.lock().unwrap().check(name, &truncate_display(&input_display, 200));

    match result {
        ConfirmResult::Approved => Ok(None),
        ConfirmResult::Denied => Ok(Some(ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: "Tool execution denied by user".to_string(),
            is_error: true,
        })),
        ConfirmResult::Quit => Err(ExecutionControl::Quit),
    }
}

async fn execute_single(
    registry: &ToolRegistry,
    call: &ContentBlock,
    hooks: Option<&HookEngine>,
) -> ContentBlock {
    let ContentBlock::ToolUse { id, name, input } = call else {
        unreachable!("execute_single called with non-ToolUse block")
    };

    // Run pre-tool-use hooks
    if let Some(hook_engine) = hooks {
        if let Err(e) = hook_engine.run_pre_tool_use(name, input).await {
            return ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: format!("Blocked by hook: {}", e),
                is_error: true,
            };
        }
    }

    let result = match registry.get(name) {
        Some(tool) => {
            let max_size = tool.max_result_size();
            let r = tool.execute(input.clone()).await;
            ToolResult {
                content: truncate_result(&r.content, max_size),
                is_error: r.is_error,
            }
        }
        None => ToolResult {
            content: format!("Unknown tool: {}", name),
            is_error: true,
        },
    };

    // Run post-tool-use hooks
    if let Some(hook_engine) = hooks {
        let messages = hook_engine
            .run_post_tool_use(name, input, &result.content)
            .await;
        for msg in messages {
            eprintln!("{}", msg);
        }
    }

    ContentBlock::ToolResult {
        tool_use_id: id.clone(),
        content: result.content,
        is_error: result.is_error,
    }
}

/// Execute tool calls with JSON stream protocol approval flow
pub async fn execute_tool_calls_with_approval(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    approval_manager: &Arc<ToolApprovalManager>,
    writer: &Arc<ProtocolWriter>,
    msg_id: &str,
    auto_approve: bool,
    allow_list: &[String],
    hooks: Option<&HookEngine>,
) -> Result<Vec<ContentBlock>, ExecutionControl> {
    let mut results = Vec::new();

    for call in tool_calls {
        let ContentBlock::ToolUse { id, name, input } = call else {
            continue;
        };

        let tool = registry.get(name);
        let category = tool.map(|t| t.category()).unwrap_or(ToolCategory::Exec);
        let description = tool.map(|t| t.describe(input)).unwrap_or_default();

        // Check if approval is needed
        let needs_approval = !auto_approve
            && !allow_list.contains(&name.to_string())
            && !approval_manager.is_auto_approved(&category.to_string());

        if needs_approval {
            // Emit tool_request and wait for approval
            writer.emit(&ProtocolEvent::ToolRequest {
                msg_id: msg_id.to_string(),
                call_id: id.clone(),
                tool: ToolInfo {
                    name: name.clone(),
                    category: category.clone(),
                    args: input.clone(),
                    description,
                },
            });

            let rx = approval_manager.request_approval(id);
            match rx.await {
                Ok(ToolApprovalResult::Approved) => { /* continue to execute */ }
                Ok(ToolApprovalResult::Denied { reason }) => {
                    writer.emit(&ProtocolEvent::ToolCancelled {
                        msg_id: msg_id.to_string(),
                        call_id: id.clone(),
                        reason: reason.clone(),
                    });
                    results.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: format!("Tool denied: {reason}"),
                        is_error: true,
                    });
                    continue;
                }
                Err(_) => {
                    // Channel dropped — client disconnected
                    return Err(ExecutionControl::Quit);
                }
            }
        }

        // Emit tool_running
        writer.emit(&ProtocolEvent::ToolRunning {
            msg_id: msg_id.to_string(),
            call_id: id.clone(),
            tool_name: name.clone(),
        });

        // Execute the tool
        let result = execute_single(registry, call, hooks).await;

        // Emit tool_result event
        if let ContentBlock::ToolResult { content, is_error, .. } = &result {
            let status = if *is_error { ToolStatus::Error } else { ToolStatus::Success };
            writer.emit(&ProtocolEvent::ToolResult {
                msg_id: msg_id.to_string(),
                call_id: id.clone(),
                tool_name: name.clone(),
                status,
                output: content.clone(),
                output_type: OutputType::Text,
                metadata: None,
            });
        }

        results.push(result);
    }

    Ok(results)
}

fn truncate_result(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let half = max_chars / 2;
    let head = &content[..half];
    let tail = &content[content.len() - half..];
    format!(
        "{}\n\n... [truncated {} chars] ...\n\n{}",
        head,
        content.len() - max_chars,
        tail
    )
}

fn truncate_display(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

struct Batch<'a> {
    is_concurrent: bool,
    calls: Vec<&'a ContentBlock>,
}

fn partition<'a>(registry: &ToolRegistry, calls: &'a [ContentBlock]) -> Vec<Batch<'a>> {
    let mut batches: Vec<Batch<'a>> = Vec::new();

    for call in calls {
        let ContentBlock::ToolUse { name, input, .. } = call else {
            continue;
        };
        let is_safe = registry
            .get(name)
            .map(|t| t.is_concurrency_safe(input))
            .unwrap_or(false);

        match batches.last_mut() {
            Some(last) if last.is_concurrent && is_safe => {
                last.calls.push(call);
            }
            _ => {
                batches.push(Batch {
                    is_concurrent: is_safe,
                    calls: vec![call],
                });
            }
        }
    }

    batches
}
