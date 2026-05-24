use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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
    pub fn new(config: Config, workspace: impl Into<String>, output: Arc<dyn OutputSink>) -> Self {
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
    pub async fn build(mut self) -> anyhow::Result<BootstrapResult> {
        let cwd = &self.workspace;
        let cwd_path = std::path::Path::new(cwd);

        let provider = self
            .provider
            .unwrap_or_else(|| aion_providers::create_provider(&self.config));

        let memory_dir = aion_memory::paths::auto_memory_dir(cwd_path);

        let file_cache = if self.config.file_cache.enabled {
            Some(Arc::new(std::sync::RwLock::new(
                aion_tools::file_cache::FileStateCache::new(&self.config.file_cache),
            )))
        } else {
            None
        };

        let mut registry = aion_tools::registry::ToolRegistry::new();
        registry.register(Box::new(aion_tools::read::ReadTool::new(
            file_cache.clone(),
        )));
        registry.register(Box::new(aion_tools::write::WriteTool::new(
            file_cache.clone(),
        )));
        registry.register(Box::new(aion_tools::edit::EditTool::new(file_cache)));
        registry.register(Box::new(aion_tools::bash::BashTool::new(
            cwd_path.to_path_buf(),
        )));
        registry.register(Box::new(aion_tools::grep::GrepTool::new(
            cwd_path.to_path_buf(),
        )));
        registry.register(Box::new(aion_tools::glob::GlobTool::new(
            cwd_path.to_path_buf(),
        )));

        let builtin_names: Vec<String> = registry.tool_names();

        let mut mcp_managers: Vec<Arc<McpManager>> = Vec::new();
        let mcp_manager = if !self.config.mcp.servers.is_empty() {
            match McpManager::connect_all(&self.config.mcp.servers).await {
                Ok(mgr) => {
                    let mgr = Arc::new(mgr);
                    aion_mcp::tool_proxy::register_mcp_tools(
                        &mut registry,
                        &mgr,
                        &builtin_names,
                        &self.config.mcp.servers,
                    );
                    mcp_managers.push(mgr.clone());
                    Some(mgr)
                }
                Err(e) => {
                    self.output
                        .emit_error(&format!("MCP initialization error: {e}"));
                    None
                }
            }
        } else {
            None
        };
        let has_mcp = mcp_manager.is_some();

        let skills = aion_skills::loader::load_all_skills(
            cwd_path,
            &self.extra_skill_dirs,
            false,
            mcp_manager.as_deref(),
        )
        .await;

        let mut prompt_cache = crate::context::SystemPromptCache::new();
        let system_prompt = crate::context::build_system_prompt(
            &mut prompt_cache,
            self.config.system_prompt.as_deref(),
            cwd,
            &self.config.model,
            &skills,
            None,
            memory_dir.as_deref(),
            false,
            self.config.compact.toon,
        );
        self.config.system_prompt = Some(system_prompt);

        let skills_arc = Arc::new(skills);
        let skill_checker = aion_skills::permissions::SkillPermissionChecker::new(
            self.config.tools.skills.deny.clone(),
            self.config.tools.skills.allow.clone(),
            self.config.tools.auto_approve,
        );
        registry.register(Box::new(crate::skill_tool::SkillTool::new(
            skills_arc,
            cwd.to_string(),
            skill_checker,
        )));

        let spawner = Arc::new(crate::spawner::AgentSpawner::new(
            provider.clone(),
            self.config.clone(),
            cwd_path.to_path_buf(),
        ));
        registry.register(Box::new(crate::spawn_tool::SpawnTool::new(spawner)));

        let plan_active_flag = Arc::new(AtomicBool::new(false));
        if self.config.plan.enabled {
            registry.register(Box::new(crate::plan::tools::EnterPlanModeTool::new(
                Arc::clone(&plan_active_flag),
            )));
            registry.register(Box::new(crate::plan::tools::ExitPlanModeTool::new(
                Arc::clone(&plan_active_flag),
            )));
        }

        let tool_defs_snapshot = registry.to_tool_defs();
        registry.register(Box::new(aion_tools::tool_search::ToolSearchTool::new(
            tool_defs_snapshot,
        )));

        let mut engine = if let Some(session) = self.resume_session {
            AgentEngine::resume_with_provider(
                provider.clone(),
                self.config,
                registry,
                self.output,
                session,
                cwd_path.to_path_buf(),
            )
        } else {
            AgentEngine::new_with_provider(
                provider.clone(),
                self.config,
                registry,
                self.output,
                cwd_path.to_path_buf(),
            )
        };
        engine.set_plan_active_flag(plan_active_flag);

        Ok(BootstrapResult {
            engine,
            provider,
            mcp_managers,
            has_mcp,
        })
    }
}
