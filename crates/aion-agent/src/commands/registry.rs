use std::sync::Arc;

use async_trait::async_trait;

use super::{clear, compact, context, help, quit};
use crate::compact::state::CompactState;
use crate::context_usage::{ContextState, PromptUsage};
use crate::output::OutputSink;
use aion_config::compact::CompactConfig;
use aion_providers::LlmProvider;
use aion_types::message::Message;
use aion_types::tool::ToolDef;

/// Result of executing a slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandResult {
    /// Command handled, continue the REPL loop.
    Continue,
    /// Command mutated context and the updated session must be persisted.
    ContextChanged,
    /// Exit the REPL.
    Exit,
}

/// Context passed to slash commands during execution.
pub struct CommandContext<'a> {
    pub messages: &'a mut Vec<Message>,
    pub compact_state: &'a mut CompactState,
    pub compact_config: &'a CompactConfig,
    pub provider: Arc<dyn LlmProvider>,
    pub model: &'a str,
    pub output: &'a dyn OutputSink,
    pub registry: &'a CommandRegistry,
    pub(crate) context_state: &'a mut ContextState,
    pub(crate) prompt_usage: &'a PromptUsage,
    pub(crate) context_tools: &'a [ToolDef],
    pub(crate) dynamic_system_tokens: u64,
}

/// A slash command that can be executed in the REPL.
#[async_trait]
pub trait SlashCommand: Send + Sync {
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&str] {
        &[]
    }
    fn description(&self) -> &str;
    async fn execute(&self, ctx: &mut CommandContext<'_>, args: &str) -> anyhow::Result<CommandResult>;
}

/// Registry of all available slash commands.
pub struct CommandRegistry {
    commands: Vec<Box<dyn SlashCommand>>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self { commands: Vec::new() }
    }

    pub fn register(&mut self, cmd: Box<dyn SlashCommand>) {
        self.commands.push(cmd);
    }

    pub fn find(&self, name: &str) -> Option<&dyn SlashCommand> {
        self.commands.iter().find_map(|cmd| {
            if cmd.name() == name || cmd.aliases().contains(&name) {
                Some(cmd.as_ref())
            } else {
                None
            }
        })
    }

    pub fn all(&self) -> &[Box<dyn SlashCommand>] {
        &self.commands
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the default registry with all built-in commands.
pub fn default_registry() -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    registry.register(Box::new(compact::CompactCommand));
    registry.register(Box::new(context::ContextCommand));
    registry.register(Box::new(clear::ClearCommand));
    registry.register(Box::new(help::HelpCommand));
    registry.register(Box::new(quit::QuitCommand));
    registry
}

#[cfg(test)]
#[path = "registry_test.rs"]
mod registry_test;
