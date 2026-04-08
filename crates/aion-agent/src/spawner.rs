use std::sync::Arc;

use async_trait::async_trait;

use aion_config::config::Config;
use aion_providers::LlmProvider;
use aion_tools::bash::BashTool;
use aion_tools::edit::EditTool;
use aion_tools::glob::GlobTool;
use aion_tools::grep::GrepTool;
use aion_tools::read::ReadTool;
use aion_tools::registry::ToolRegistry;
use aion_tools::write::WriteTool;
use aion_types::message::TokenUsage;

use crate::engine::AgentEngine;
use crate::output::OutputSink;
use crate::output::terminal::TerminalSink;

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

/// Additional overrides applied when spawning a fork-mode skill sub-agent.
#[derive(Debug, Clone, Default)]
pub struct ForkOverrides {
    /// Replace the parent's configured model with this one.
    pub model: Option<String>,
    /// Reasoning effort ("low"/"medium"/"high"/"max").
    pub effort: Option<String>,
    /// Restrict registered tools to this list; empty = all built-in tools.
    pub allowed_tools: Vec<String>,
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

/// Abstraction over fork-mode agent spawning — enables mock implementations in tests.
#[async_trait]
pub trait Spawner: Send + Sync {
    /// Spawn a fork-mode sub-agent with optional skill overrides and wait for its result.
    async fn spawn_fork(
        &self,
        config: SubAgentConfig,
        overrides: ForkOverrides,
    ) -> SubAgentResult;
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
        config.session.enabled = false;
        config.tools.auto_approve = true;

        let tools = build_tool_registry(&[]);
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

#[async_trait]
impl Spawner for AgentSpawner {
    /// Spawn a fork-mode sub-agent applying skill-level overrides.
    async fn spawn_fork(
        &self,
        sub_config: SubAgentConfig,
        overrides: ForkOverrides,
    ) -> SubAgentResult {
        let mut config = self.base_config.clone();
        config.max_turns = sub_config.max_turns;
        config.max_tokens = sub_config.max_tokens;
        if let Some(sp) = sub_config.system_prompt {
            config.system_prompt = Some(sp);
        }
        config.session.enabled = false;
        config.tools.auto_approve = true;

        if let Some(model) = overrides.model.clone() {
            config.model = model;
        }

        let tools = build_tool_registry(&overrides.allowed_tools);
        let output: Arc<dyn OutputSink> = Arc::new(TerminalSink::new(true));

        let mut engine =
            AgentEngine::new_with_provider(self.provider.clone(), config, tools, output);

        engine.set_initial_reasoning_effort(overrides.effort.clone());

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
}

type ToolFactory = fn() -> Box<dyn aion_tools::Tool>;

/// Build a fresh tool registry for a sub-agent.
fn build_tool_registry(allowed: &[String]) -> ToolRegistry {
    let all: &[(&str, ToolFactory)] = &[
        ("Read", || Box::new(ReadTool)),
        ("Write", || Box::new(WriteTool)),
        ("Edit", || Box::new(EditTool)),
        ("Bash", || Box::new(BashTool)),
        ("Grep", || Box::new(GrepTool)),
        ("Glob", || Box::new(GlobTool)),
    ];

    let mut registry = ToolRegistry::new();
    for (name, make_tool) in all {
        if allowed.is_empty() || allowed.iter().any(|a| a.as_str() == *name) {
            registry.register(make_tool());
        }
    }
    registry
}

// ---------------------------------------------------------------------------
// Phase 7 tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod phase7_tests {
    use super::{ForkOverrides, SubAgentConfig, build_tool_registry};

    #[test]
    fn tc_7_1_fork_overrides_default_values() {
        let overrides = ForkOverrides::default();
        assert!(overrides.model.is_none());
        assert!(overrides.effort.is_none());
        assert!(overrides.allowed_tools.is_empty());
    }

    #[test]
    fn tc_7_2_fork_overrides_model_set() {
        let overrides = ForkOverrides {
            model: Some("claude-opus-4-6".to_string()),
            ..Default::default()
        };
        assert_eq!(overrides.model.as_deref(), Some("claude-opus-4-6"));
    }

    #[test]
    fn tc_7_3_fork_overrides_effort_set() {
        let overrides = ForkOverrides {
            effort: Some("high".to_string()),
            ..Default::default()
        };
        assert_eq!(overrides.effort.as_deref(), Some("high"));
    }

    #[test]
    fn tc_7_4_fork_overrides_allowed_tools_set() {
        let overrides = ForkOverrides {
            allowed_tools: vec!["Bash".to_string(), "Read".to_string()],
            ..Default::default()
        };
        assert_eq!(overrides.allowed_tools, vec!["Bash", "Read"]);
    }

    #[test]
    fn tc_7_5_fork_overrides_all_fields_together() {
        let overrides = ForkOverrides {
            model: Some("claude-sonnet-4-6".to_string()),
            effort: Some("low".to_string()),
            allowed_tools: vec!["Write".to_string()],
        };
        assert_eq!(overrides.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(overrides.effort.as_deref(), Some("low"));
        assert_eq!(overrides.allowed_tools, vec!["Write"]);
    }

    #[test]
    fn tc_7_6_fork_overrides_clone_preserves_fields() {
        let original = ForkOverrides {
            model: Some("my-model".to_string()),
            effort: Some("max".to_string()),
            allowed_tools: vec!["Bash".to_string()],
        };
        let cloned = original.clone();
        assert_eq!(cloned.model, original.model);
        assert_eq!(cloned.effort, original.effort);
        assert_eq!(cloned.allowed_tools, original.allowed_tools);
    }

    #[test]
    fn tc_7_40_build_tool_registry_empty_allowed_registers_all() {
        let registry = build_tool_registry(&[]);
        for name in &["Read", "Write", "Edit", "Bash", "Grep", "Glob"] {
            assert!(registry.get(name).is_some(), "tool '{name}' should be registered");
        }
    }

    #[test]
    fn tc_7_43_build_tool_registry_filters_to_allowed() {
        let allowed = vec!["Bash".to_string(), "Read".to_string()];
        let registry = build_tool_registry(&allowed);
        assert!(registry.get("Bash").is_some());
        assert!(registry.get("Read").is_some());
        assert!(registry.get("Write").is_none());
        assert!(registry.get("Edit").is_none());
    }

    #[test]
    fn tc_7_43b_build_tool_registry_single_tool() {
        let allowed = vec!["Glob".to_string()];
        let registry = build_tool_registry(&allowed);
        assert!(registry.get("Glob").is_some());
        assert!(registry.get("Bash").is_none());
    }

    #[test]
    fn tc_7_sub_agent_config_original_fields_intact() {
        let config = SubAgentConfig {
            name: "test-agent".to_string(),
            prompt: "do the task".to_string(),
            max_turns: 5,
            max_tokens: 1024,
            system_prompt: Some("you are helpful".to_string()),
        };
        assert_eq!(config.name, "test-agent");
        assert_eq!(config.max_turns, 5);
    }
}
