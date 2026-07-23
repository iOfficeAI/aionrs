use std::cmp::Reverse;
use std::fmt::Write;

use async_trait::async_trait;

use super::{CommandContext, CommandResult, SlashCommand};
use crate::context_usage::{ContextBreakdown, ContextSnapshot, ContextUsageSource, MessageUsage, NamedUsage};

const BAR_WIDTH: usize = 20;
const DETAIL_MESSAGE_LIMIT: usize = 10;

pub(super) struct ContextCommand;

#[async_trait]
impl SlashCommand for ContextCommand {
    fn name(&self) -> &str {
        "context"
    }

    fn description(&self) -> &str {
        "Show current context usage"
    }

    async fn execute(&self, ctx: &mut CommandContext<'_>, args: &str) -> anyhow::Result<CommandResult> {
        let expanded = match args.trim() {
            "" => false,
            "all" => true,
            other => {
                ctx.output.emit_error(&format!(
                    "Unknown /context argument: {other}. Use /context or /context all."
                ));
                return Ok(CommandResult::Continue);
            }
        };
        let snapshot = ContextSnapshot::build(
            ctx.model,
            ctx.compact_config.context_window,
            ctx.context_state,
            ctx.prompt_usage,
            ctx.dynamic_system_tokens,
            ctx.context_tools,
            ctx.messages,
        );
        ctx.output.emit_info(&format_snapshot(&snapshot, expanded));
        Ok(CommandResult::Continue)
    }
}

fn format_snapshot(snapshot: &ContextSnapshot, expanded: bool) -> String {
    let usage_pct = percent(snapshot.context_usage, snapshot.context_window);
    let free_tokens = snapshot.context_window.saturating_sub(snapshot.context_usage);
    let free_pct = percent(free_tokens, snapshot.context_window);
    let source = match snapshot.source {
        ContextUsageSource::ProviderExact => "provider exact",
        ContextUsageSource::LocalProjected => "local projected",
    };

    let mut output = String::new();
    writeln!(&mut output, "Context Usage").expect("writing to String cannot fail");
    writeln!(&mut output, "  Model: {}", snapshot.model).expect("writing to String cannot fail");
    writeln!(
        &mut output,
        "  [{}]",
        usage_bar(snapshot.context_usage, snapshot.context_window)
    )
    .expect("writing to String cannot fail");
    writeln!(
        &mut output,
        "  {}/{} tokens ({usage_pct:.1}%)",
        format_tokens(snapshot.context_usage),
        format_tokens(snapshot.context_window)
    )
    .expect("writing to String cannot fail");
    writeln!(
        &mut output,
        "  Free space: {} tokens ({free_pct:.1}%)",
        format_tokens(free_tokens)
    )
    .expect("writing to String cannot fail");
    writeln!(&mut output, "  Source: {source}").expect("writing to String cannot fail");
    writeln!(
        &mut output,
        "  Compactions: {} compact, {} microcompact",
        snapshot.compact_count, snapshot.microcompact_count
    )
    .expect("writing to String cannot fail");
    writeln!(&mut output, "  Updated: {}", snapshot.updated_at.to_rfc3339()).expect("writing to String cannot fail");
    writeln!(&mut output).expect("writing to String cannot fail");
    writeln!(&mut output, "Estimated usage by category").expect("writing to String cannot fail");
    write_breakdown(&mut output, &snapshot.breakdown, snapshot.context_window);

    if expanded {
        write_expanded_details(&mut output, snapshot);
    } else {
        writeln!(&mut output).expect("writing to String cannot fail");
        write!(&mut output, "  /context all to expand").expect("writing to String cannot fail");
    }

    output
}

fn write_breakdown(output: &mut String, breakdown: &ContextBreakdown, context_window: u64) {
    let entries = [
        ("System prompt", breakdown.system_prompt),
        ("Memory", breakdown.memory),
        ("Skills", breakdown.skills),
        ("Tools", breakdown.tools),
        ("Messages", breakdown.messages),
        ("Unattributed", breakdown.unattributed),
    ];
    for (label, tokens) in entries {
        writeln!(
            output,
            "  {label}: {} tokens ({:.1}%)",
            format_tokens(tokens),
            percent(tokens, context_window)
        )
        .expect("writing to String cannot fail");
    }
}

fn write_expanded_details(output: &mut String, snapshot: &ContextSnapshot) {
    write_named_usage(output, "Memory files", &snapshot.memory_files);

    writeln!(output).expect("writing to String cannot fail");
    writeln!(output, "Skills · {}", snapshot.skills.len()).expect("writing to String cannot fail");
    if snapshot.skills.is_empty() {
        writeln!(output, "  (none)").expect("writing to String cannot fail");
    } else {
        for skill in &snapshot.skills {
            writeln!(output, "  - {skill}").expect("writing to String cannot fail");
        }
    }

    write_named_usage(output, "Tools", &snapshot.tools);

    let mut messages = snapshot.messages.clone();
    messages.sort_by_key(|message| Reverse(message.tokens));
    messages.truncate(DETAIL_MESSAGE_LIMIT);
    writeln!(output).expect("writing to String cannot fail");
    writeln!(output, "Largest messages · showing {}", messages.len()).expect("writing to String cannot fail");
    if messages.is_empty() {
        writeln!(output, "  (none)").expect("writing to String cannot fail");
    } else {
        for message in messages {
            write_message_usage(output, &message);
        }
    }
}

fn write_named_usage(output: &mut String, heading: &str, items: &[NamedUsage]) {
    writeln!(output).expect("writing to String cannot fail");
    writeln!(output, "{heading} · {}", items.len()).expect("writing to String cannot fail");
    if items.is_empty() {
        writeln!(output, "  (none)").expect("writing to String cannot fail");
    } else {
        for item in items {
            writeln!(output, "  - {} · {} tokens", item.name, format_tokens(item.tokens))
                .expect("writing to String cannot fail");
        }
    }
}

fn write_message_usage(output: &mut String, message: &MessageUsage) {
    writeln!(
        output,
        "  - #{} {:?} · {} tokens",
        message.index + 1,
        message.role,
        format_tokens(message.tokens)
    )
    .expect("writing to String cannot fail");
}

fn usage_bar(usage: u64, context_window: u64) -> String {
    let filled = if context_window == 0 {
        0
    } else {
        ((usage.min(context_window) as u128 * BAR_WIDTH as u128) / context_window as u128) as usize
    };
    format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled))
}

fn percent(tokens: u64, context_window: u64) -> f64 {
    if context_window == 0 {
        0.0
    } else {
        tokens as f64 * 100.0 / context_window as f64
    }
}

fn format_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }
    let thousands = tokens as f64 / 1_000.0;
    if tokens.is_multiple_of(1_000) {
        format!("{thousands:.0}k")
    } else {
        format!("{thousands:.1}k")
    }
}

#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;
