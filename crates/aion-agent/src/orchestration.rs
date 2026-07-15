use std::sync::{Arc, Mutex};

use crate::confirm::{ConfirmResult, ToolConfirmer};
use aion_config::hooks::HookEngine;
use aion_protocol::events::{OutputType, ProtocolEvent, ToolCategory, ToolInfo, ToolStatus};
use aion_protocol::writer::ProtocolEmitter;
use aion_protocol::{ToolApprovalManager, ToolApprovalResult};
use aion_types::message::ContentBlock;
use aion_types::skill_types::ContextModifier;
use aion_types::tool::ToolResult;

use aion_tools::registry::ToolRegistry;

/// The combined output of a tool execution batch: protocol content blocks
/// paired with per-call context modifiers (None for non-skill tools).
pub struct ToolCallOutcome {
    pub results: Vec<ContentBlock>,
    pub modifiers: Vec<Option<ContextModifier>>,
    pub follow_up_blocks: Vec<ContentBlock>,
}

impl std::ops::Deref for ToolCallOutcome {
    type Target = Vec<ContentBlock>;
    fn deref(&self) -> &Self::Target {
        &self.results
    }
}

impl std::ops::DerefMut for ToolCallOutcome {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.results
    }
}

/// Partition tool calls and execute them with optional confirmation and hooks
pub async fn execute_tool_calls(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    mut hooks: Option<&mut HookEngine>,
    compaction_level: aion_compact::CompactLevel,
    toon_enabled: bool,
) -> Result<ToolCallOutcome, ExecutionControl> {
    let mut results = Vec::new();
    let mut modifiers = Vec::new();
    let mut follow_up_blocks = Vec::new();

    for batch in partition(registry, tool_calls) {
        if batch.is_concurrent {
            // For concurrent batch, confirm all first, then execute approved ones.
            // Concurrent tools are never SkillTool (is_concurrency_safe=false for Skill),
            // so no skill hooks merging is needed here.
            let mut approved = Vec::new();
            for call in &batch.calls {
                match confirm_call(confirmer, call)? {
                    Some(denied) => {
                        results.push(denied);
                        modifiers.push(None);
                    }
                    None => approved.push(call),
                }
            }
            // Reborrow as shared for concurrent execution.
            let hooks_shared: Option<&HookEngine> = hooks.as_deref();
            let futures: Vec<_> = approved
                .iter()
                .map(|call| execute_single(registry, call, hooks_shared, compaction_level, toon_enabled))
                .collect();
            let batch_results = futures::future::join_all(futures).await;
            for (block, modifier, blocks) in batch_results {
                results.push(block);
                modifiers.push(modifier);
                follow_up_blocks.extend(blocks);
            }
        } else {
            for call in &batch.calls {
                match confirm_call(confirmer, call)? {
                    Some(denied) => {
                        results.push(denied);
                        modifiers.push(None);
                    }
                    None => {
                        // Reborrow as shared for execute_single, then reclaim mut for merge.
                        let block;
                        let modifier;
                        let blocks;
                        {
                            let hooks_shared: Option<&HookEngine> = hooks.as_deref();
                            (block, modifier, blocks) =
                                execute_single(registry, call, hooks_shared, compaction_level, toon_enabled).await;
                        }
                        // Merge skill hooks after a successful sequential execution.
                        if !block_is_error(&block) {
                            maybe_merge_skill_hooks(registry, call, hooks.as_deref_mut());
                        }
                        results.push(block);
                        modifiers.push(modifier);
                        follow_up_blocks.extend(blocks);
                    }
                }
            }
        }
    }

    Ok(ToolCallOutcome {
        results,
        modifiers,
        follow_up_blocks,
    })
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
    let ContentBlock::ToolUse { id, name, input, .. } = call else {
        return Ok(None);
    };

    let input_display = serde_json::to_string(input).unwrap_or_default();
    let result = confirmer
        .lock()
        .unwrap()
        .check(name, &truncate_display(&input_display, 200));

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
    compaction_level: aion_compact::CompactLevel,
    toon_enabled: bool,
) -> (ContentBlock, Option<ContextModifier>, Vec<ContentBlock>) {
    let ContentBlock::ToolUse { id, name, input, .. } = call else {
        unreachable!("execute_single called with non-ToolUse block")
    };

    let start = std::time::Instant::now();
    tracing::info!(target: "aion_agent", tool = %name, call_id = %id, "tool execution started");

    // Run pre-tool-use hooks
    if let Some(hook_engine) = hooks
        && let Err(e) = hook_engine.run_pre_tool_use(name, input).await
    {
        return (
            ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: format!("Blocked by hook: {}", e),
                is_error: true,
            },
            None,
            Vec::new(),
        );
    }

    let (result, modifier, follow_up_blocks) = match registry.get(name) {
        Some(tool) => {
            let max_size = tool.max_result_size();
            let execution = tool.execute_with_follow_up(input.clone()).await;
            let r = execution.result;
            let modifier = if r.is_error {
                None
            } else {
                tool.context_modifier_for(input)
            };
            let follow_up_blocks = if r.is_error {
                Vec::new()
            } else {
                execution.follow_up_blocks
            };
            let error_content = if r.is_error && tool.is_deferred() {
                maybe_append_deferred_hint(&r.content, tool.input_schema(), input)
            } else {
                r.content.clone()
            };
            let content = truncate_result(&error_content, max_size);
            let content = aion_compact::compact_output(&content, compaction_level);
            let content = if toon_enabled {
                aion_compact::compact_output_toon(&content)
            } else {
                content
            };
            (
                ToolResult {
                    content,
                    is_error: r.is_error,
                },
                modifier,
                follow_up_blocks,
            )
        }
        None => (
            ToolResult {
                content: format!("Unknown tool: {}", name),
                is_error: true,
            },
            None,
            Vec::new(),
        ),
    };

    // Run post-tool-use hooks
    if let Some(hook_engine) = hooks {
        let messages = hook_engine.run_post_tool_use(name, input, &result.content).await;
        for msg in messages {
            tracing::info!(target: "aion_agent", hook_message = %msg, "post-tool-use hook output");
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    tracing::info!(target: "aion_agent", duration_ms, success = !result.is_error, "tool execution completed");

    (
        ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: result.content,
            is_error: result.is_error,
        },
        modifier,
        follow_up_blocks,
    )
}

/// Execute tool calls with JSON stream protocol approval flow
#[allow(clippy::too_many_arguments)]
pub async fn execute_tool_calls_with_approval(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    approval_manager: &Arc<ToolApprovalManager>,
    writer: &Arc<dyn ProtocolEmitter>,
    msg_id: &str,
    auto_approve: bool,
    allow_list: &[String],
    mut hooks: Option<&mut HookEngine>,
    compaction_level: aion_compact::CompactLevel,
    toon_enabled: bool,
) -> Result<ToolCallOutcome, ExecutionControl> {
    let mut results = Vec::new();
    let mut modifiers = Vec::new();
    let mut follow_up_blocks = Vec::new();

    for call in tool_calls {
        let ContentBlock::ToolUse { id, name, input, .. } = call else {
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
            let _ = writer.emit(&ProtocolEvent::ToolRequest {
                msg_id: msg_id.to_string(),
                call_id: id.clone(),
                tool: ToolInfo {
                    name: name.clone(),
                    category,
                    args: input.clone(),
                    description,
                },
            });

            let rx = approval_manager.request_approval(id, &category);
            match rx.await {
                Ok(ToolApprovalResult::Approved) => { /* continue to execute */ }
                Ok(ToolApprovalResult::Denied { reason }) => {
                    let _ = writer.emit(&ProtocolEvent::ToolCancelled {
                        msg_id: msg_id.to_string(),
                        call_id: id.clone(),
                        reason: reason.clone(),
                    });
                    results.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: format!("Tool denied: {reason}"),
                        is_error: true,
                    });
                    modifiers.push(None);
                    continue;
                }
                Err(_) => {
                    // Channel dropped — client disconnected
                    return Err(ExecutionControl::Quit);
                }
            }
        }

        // Emit tool_running
        let _ = writer.emit(&ProtocolEvent::ToolRunning {
            msg_id: msg_id.to_string(),
            call_id: id.clone(),
            tool_name: name.clone(),
        });

        // Execute the tool (reborrow as shared for execute_single, then reclaim mut for merge).
        let result;
        let modifier;
        let blocks;
        {
            let hooks_shared: Option<&HookEngine> = hooks.as_deref();
            (result, modifier, blocks) =
                execute_single(registry, call, hooks_shared, compaction_level, toon_enabled).await;
        }

        // Emit tool_result event
        if let ContentBlock::ToolResult { content, is_error, .. } = &result {
            let status = if *is_error {
                ToolStatus::Error
            } else {
                ToolStatus::Success
            };
            let _ = writer.emit(&ProtocolEvent::ToolResult {
                msg_id: msg_id.to_string(),
                call_id: id.clone(),
                tool_name: name.clone(),
                status,
                output: content.clone(),
                output_type: OutputType::Text,
                metadata: None,
            });
        }

        // Merge skill hooks after a successful execution.
        if !block_is_error(&result) {
            maybe_merge_skill_hooks(registry, call, hooks.as_deref_mut());
        }

        results.push(result);
        modifiers.push(modifier);
        follow_up_blocks.extend(blocks);
    }

    Ok(ToolCallOutcome {
        results,
        modifiers,
        follow_up_blocks,
    })
}

/// If `call` is a Skill tool call that returned successfully, merge skill hooks into the engine.
fn merge_skill_hooks_into(engine: &mut HookEngine, registry: &ToolRegistry, call: &ContentBlock) {
    let ContentBlock::ToolUse { name, input, .. } = call else {
        return;
    };
    if name != "Skill" {
        return;
    }
    let Some(tool) = registry.get(name) else {
        return;
    };
    if let Some(skill_hooks) = tool.skill_hooks_for(input) {
        engine.merge_hooks(skill_hooks);
    }
}

fn maybe_merge_skill_hooks(registry: &ToolRegistry, call: &ContentBlock, hooks: Option<&mut HookEngine>) {
    if let Some(engine) = hooks {
        merge_skill_hooks_into(engine, registry, call);
    }
}

/// Returns true when a ContentBlock::ToolResult has is_error=true.
fn block_is_error(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::ToolResult { is_error: true, .. })
}

/// When a deferred tool fails AND the input is missing required fields from
/// its full schema, append a hint telling the LLM to call ToolSearch first.
/// If required fields are all present (or the schema has none), the original
/// error is returned unchanged — the failure is a runtime issue, not a
/// missing-schema problem.
fn maybe_append_deferred_hint(original_error: &str, schema: serde_json::Value, input: &serde_json::Value) -> String {
    let missing: Vec<&str> = schema["required"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter(|key| input.get(key).is_none())
                .collect()
        })
        .unwrap_or_default();

    if missing.is_empty() {
        return original_error.to_string();
    }

    format!(
        "{}\n\nThis is a deferred tool — its full parameter schema was not loaded. \
         Call ToolSearch to load the schema, then retry.",
        original_error
    )
}

fn truncate_result(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let half = max_chars / 2;
    // Find char boundaries to avoid panicking on multi-byte characters
    let head_end = content
        .char_indices()
        .nth(half)
        .map(|(i, _)| i)
        .unwrap_or(content.len());
    let tail_start = content.char_indices().rev().nth(half - 1).map(|(i, _)| i).unwrap_or(0);
    let head = &content[..head_end];
    let tail = &content[tail_start..];
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
        // Find a char boundary to avoid panicking on multi-byte characters
        let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}...", &s[..end])
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

#[cfg(test)]
#[path = "orchestration_test.rs"]
mod orchestration_test;
