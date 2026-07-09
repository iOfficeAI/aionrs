//! Shared initialization steps for aion-cli entry points.
//!
//! `run.rs` (terminal REPL) and `json_stream` (host-integration protocol)
//! both need to resolve config, initialize logging, and bootstrap an
//! `AgentEngine` (optionally resuming a saved session). This module holds
//! that shared logic so the two call sites don't duplicate it.
//!
//! Note: `aion_agent` also exposes a module named `bootstrap`
//! (`aion_agent::bootstrap::AgentBootstrap`). Call sites should keep using
//! that fully-qualified path for the engine builder itself; this module is
//! `crate::bootstrap` and only wraps it.

use std::sync::Arc;

use aion_agent::bootstrap::{AgentBootstrap, BootstrapResult};
use aion_agent::output::OutputSink;
use aion_agent::session::{Session, SessionManager};
use aion_compact::CompactLevel;
use aion_config::config::{CliArgs, Config};
use aion_config::logging::{LoggingGuard, create_file_layer};

use crate::cli::Cli;

/// Resolve layered config (files + CLI args + env vars), then apply
/// CLI-only overrides that don't have a `CliArgs` slot (compaction level,
/// TOON encoding).
pub(crate) fn resolve_config(cli: &Cli) -> anyhow::Result<Config> {
    let cli_args = CliArgs {
        provider: cli.provider.clone(),
        api_key: cli.api_key.clone(),
        base_url: cli.base_url.clone(),
        model: cli.model.clone(),
        max_tokens: cli.max_tokens,
        thinking: cli.thinking.clone(),
        max_turns: cli.max_turns,
        max_tool_call_malformed_turns: cli.max_tool_call_malformed_turns,
        max_tool_call_failure_turns: cli.max_tool_call_failure_turns,
        system_prompt: cli.system_prompt.clone(),
        profile: cli.profile.clone(),
        auto_approve: cli.auto_approve,
        project_dir: cli.project_dir.clone(),
    };

    let mut config = Config::resolve(&cli_args)?;

    if let Some(ref level_str) = cli.compaction {
        match level_str.parse::<CompactLevel>() {
            Ok(level) => config.compact.compaction = level,
            Err(e) => anyhow::bail!("Invalid --compaction value: {e}"),
        }
    }
    if cli.toon {
        config.compact.toon = true;
    }

    Ok(config)
}

/// Initialize file logging from the resolved config + CLI overrides.
///
/// Returns the worker guard that must be kept alive for the process
/// lifetime, or `None` if logging is disabled or failed to initialize.
pub(crate) fn init_logging(config: &Config, log_dir: Option<&str>, log_level: Option<&str>) -> Option<LoggingGuard> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let resolved = config.logging.resolve(log_dir, log_level);
    if !resolved.enabled {
        return None;
    }

    match create_file_layer(&resolved) {
        Ok((layer, guard)) => {
            tracing_subscriber::registry().with(layer).init();
            Some(guard)
        }
        Err(e) => {
            eprintln!("Warning: failed to initialize logging: {e}");
            None
        }
    }
}

/// Bootstrap an `AgentEngine`, optionally resuming a saved session.
///
/// Both call sites (terminal REPL, JSON stream) need identical bootstrap +
/// resume-load logic, but differ in what happens *when* a session is
/// resumed: the terminal prints a "Resumed session ..." line, JSON stream
/// mode stays silent. `on_resume` captures that difference without forcing
/// the two call sites to converge on identical behavior.
pub(crate) async fn build_engine(
    config: Config,
    cwd: &str,
    output: Arc<dyn OutputSink>,
    resume_id: Option<&str>,
    on_resume: impl FnOnce(&Session),
) -> anyhow::Result<BootstrapResult> {
    let mut agent_bootstrap = AgentBootstrap::new(config, cwd, output);

    if let Some(resume_id) = resume_id {
        let cfg = agent_bootstrap.config();
        let session_mgr = SessionManager::new(cfg.session.directory.clone().into(), cfg.session.max_sessions);
        let session = session_mgr.load(resume_id)?;
        on_resume(&session);
        agent_bootstrap = agent_bootstrap.resume(session);
    }

    agent_bootstrap.build().await
}
