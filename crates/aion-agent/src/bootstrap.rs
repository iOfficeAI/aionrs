use std::path::PathBuf;
use std::sync::Arc;

use aion_config::config::Config;
use aion_mcp::manager::McpManager;
use aion_providers::LlmProvider;

use crate::engine::AgentEngine;
use crate::output::OutputSink;
use crate::session::Session;

/// Result of bootstrapping an agent engine with all features initialized.
pub struct BootstrapResult {
    pub engine: AgentEngine,
    pub provider: Arc<dyn LlmProvider>,
    pub mcp_managers: Vec<Arc<McpManager>>,
    pub has_mcp: bool,
}

/// Builder for creating a fully-initialized `AgentEngine`.
///
/// Encapsulates the complete initialization pipeline so all consumers
/// (CLI, backend, sub-agents) get consistent behavior:
///
/// - System prompt always includes model identity, working directory, date
/// - Tool usage guidance is always injected
/// - AGENTS.md is loaded from the workspace hierarchy
/// - Skills, MCP, plan mode, spawn are enabled based on `Config` fields
pub struct AgentBootstrap {
    config: Config,
    workspace: String,
    output: Arc<dyn OutputSink>,
    provider: Option<Arc<dyn LlmProvider>>,
    resume_session: Option<Session>,
    extra_skill_dirs: Vec<PathBuf>,
}

impl AgentBootstrap {
    pub fn new(
        config: Config,
        workspace: impl Into<String>,
        output: Arc<dyn OutputSink>,
    ) -> Self {
        Self {
            config,
            workspace: workspace.into(),
            output,
            provider: None,
            resume_session: None,
            extra_skill_dirs: Vec::new(),
        }
    }

    /// Use a pre-created provider instead of creating one from config.
    pub fn provider(mut self, provider: Arc<dyn LlmProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Resume from a previously saved session.
    pub fn resume(mut self, session: Session) -> Self {
        self.resume_session = Some(session);
        self
    }

    /// Add extra directories to scan for skills.
    pub fn extra_skill_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.extra_skill_dirs = dirs;
        self
    }

    /// Read-only access to the config (for session management before build).
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Build the fully-initialized engine.
    pub async fn build(self) -> anyhow::Result<BootstrapResult> {
        todo!("Implemented in Task 2")
    }
}
