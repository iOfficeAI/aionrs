use std::sync::Arc;

use crate::config::Config;
use crate::engine::AgentEngine;
use crate::output::terminal::TerminalSink;
use crate::output::OutputSink;
use crate::provider::LlmProvider;
use crate::tools::bash::BashTool;
use crate::tools::edit::EditTool;
use crate::tools::glob::GlobTool;
use crate::tools::grep::GrepTool;
use crate::tools::read::ReadTool;
use crate::tools::registry::ToolRegistry;
use crate::tools::write::WriteTool;
use crate::types::message::TokenUsage;

/// Configuration for a sub-agent
#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    /// Descriptive name for logging
    pub name: String,
    /// The task prompt
    pub prompt: String,
    /// Max turns for this sub-agent (typically lower than main agent)
    pub max_turns: usize,
    /// Max output tokens per response
    pub max_tokens: u32,
    /// Optional system prompt override
    pub system_prompt: Option<String>,
}

/// Result from a completed sub-agent
#[derive(Debug)]
pub struct SubAgentResult {
    pub name: String,
    pub text: String,
    pub usage: TokenUsage,
    pub turns: usize,
    pub is_error: bool,
}

/// Spawns independent child agents that share the parent's LLM provider
pub struct AgentSpawner {
    provider: Arc<dyn LlmProvider>,
    base_config: Config,
}

impl AgentSpawner {
    pub fn new(provider: Arc<dyn LlmProvider>, config: Config) -> Self {
        Self {
            provider,
            base_config: config,
        }
    }

    /// Spawn a single sub-agent and wait for result
    pub async fn spawn_one(&self, sub_config: SubAgentConfig) -> SubAgentResult {
        let mut config = self.base_config.clone();
        config.max_turns = sub_config.max_turns;
        config.max_tokens = sub_config.max_tokens;
        if let Some(sp) = sub_config.system_prompt {
            config.system_prompt = Some(sp);
        }
        // Disable session saving for sub-agents
        config.session.enabled = false;
        // Auto-approve tool calls for sub-agents
        config.tools.auto_approve = true;

        let tools = build_tool_registry();
        // Sub-agents run silently (no color output, no TTY)
        let output: Arc<dyn OutputSink> = Arc::new(TerminalSink::new(true));

        let mut engine =
            AgentEngine::new_with_provider(self.provider.clone(), config, tools, output);

        match engine.run(&sub_config.prompt, "").await {
            Ok(result) => SubAgentResult {
                name: sub_config.name,
                text: result.text,
                usage: result.usage,
                turns: result.turns,
                is_error: false,
            },
            Err(e) => SubAgentResult {
                name: sub_config.name,
                text: format!("Sub-agent error: {}", e),
                usage: TokenUsage::default(),
                turns: 0,
                is_error: true,
            },
        }
    }

    /// Spawn multiple sub-agents in parallel, return all results
    pub async fn spawn_parallel(
        &self,
        sub_configs: Vec<SubAgentConfig>,
    ) -> Vec<SubAgentResult> {
        let futures: Vec<_> = sub_configs
            .into_iter()
            .map(|config| {
                let spawner = self.clone_for_spawn();
                tokio::spawn(async move { spawner.spawn_one(config).await })
            })
            .collect();

        let mut results = Vec::new();
        for future in futures {
            match future.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(SubAgentResult {
                    name: "unknown".to_string(),
                    text: format!("Task join error: {}", e),
                    usage: TokenUsage::default(),
                    turns: 0,
                    is_error: true,
                }),
            }
        }
        results
    }

    fn clone_for_spawn(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            base_config: self.base_config.clone(),
        }
    }
}

/// Build a fresh tool registry with built-in tools for a sub-agent
fn build_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool));
    registry.register(Box::new(WriteTool));
    registry.register(Box::new(EditTool));
    registry.register(Box::new(BashTool));
    registry.register(Box::new(GrepTool));
    registry.register(Box::new(GlobTool));
    registry
}
