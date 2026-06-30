use async_trait::async_trait;

use super::{CommandContext, CommandResult, SlashCommand};

pub struct HelpCommand;

#[async_trait]
impl SlashCommand for HelpCommand {
    fn name(&self) -> &str {
        "help"
    }

    fn description(&self) -> &str {
        "List available commands"
    }

    async fn execute(&self, ctx: &mut CommandContext<'_>, _args: &str) -> anyhow::Result<CommandResult> {
        let mut entries: Vec<(&str, &str)> = ctx
            .registry
            .all()
            .iter()
            .map(|cmd| (cmd.name(), cmd.description()))
            .collect();
        entries.sort_by_key(|(name, _)| *name);

        let mut output = String::from("Available commands:\n");
        for (name, desc) in entries {
            output.push_str(&format!("  /{} — {}\n", name, desc));
        }
        ctx.output.emit_info(output.trim_end());
        Ok(CommandResult::Continue)
    }
}

#[cfg(test)]
#[path = "help_test.rs"]
mod help_test;
