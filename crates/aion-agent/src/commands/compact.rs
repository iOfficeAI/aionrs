use async_trait::async_trait;

use super::{CommandContext, CommandResult, SlashCommand};
use crate::compact::auto;
use aion_types::compact::CompactTrigger;

pub struct CompactCommand;

#[async_trait]
impl SlashCommand for CompactCommand {
    fn name(&self) -> &str {
        "compact"
    }

    fn description(&self) -> &str {
        "Compress conversation context"
    }

    async fn execute(
        &self,
        ctx: &mut CommandContext<'_>,
        _args: &str,
    ) -> anyhow::Result<CommandResult> {
        if ctx.messages.len() <= 2 {
            ctx.output.emit_info("Context is already compact");
            return Ok(CommandResult::Continue);
        }

        // Reset circuit breaker — manual intent overrides protection
        ctx.compact_state.consecutive_failures = 0;

        let pre_tokens = ctx.compact_state.last_input_tokens;

        match auto::autocompact(
            ctx.provider.as_ref(),
            ctx.messages,
            ctx.model,
            ctx.compact_config,
            ctx.compact_state,
        )
        .await
        {
            Ok(result) => {
                let msgs_summarized = result.messages_summarized;
                *ctx.messages = result.messages;

                if let Some(boundary) = ctx.messages.first_mut() {
                    for block in &mut boundary.content {
                        if let aion_types::message::ContentBlock::Text { text } = block {
                            if text.starts_with(auto::BOUNDARY_PREFIX) {
                                let metadata = aion_types::compact::CompactMetadata {
                                    trigger: CompactTrigger::Manual,
                                    pre_compact_tokens: pre_tokens,
                                    messages_summarized: msgs_summarized,
                                };
                                *text = format!(
                                    "{}\n{}",
                                    auto::BOUNDARY_PREFIX,
                                    serde_json::to_string(&metadata)
                                        .expect("metadata serialization cannot fail")
                                );
                            }
                        }
                    }
                }

                ctx.output.emit_info(&format!(
                    "Context compacted: {}k → compact ({} messages summarized)",
                    pre_tokens / 1000,
                    msgs_summarized
                ));
            }
            Err(e) => {
                ctx.output.emit_error(&format!("Compact failed: {}", e));
            }
        }

        Ok(CommandResult::Continue)
    }
}
