use aion_agent::session::SessionManager;
use aion_config::config::{CliArgs, Config};

use crate::cli::SessionAction;

pub(crate) fn run(action: SessionAction) -> anyhow::Result<()> {
    match action {
        SessionAction::List => list_sessions(),
    }
}

fn list_sessions() -> anyhow::Result<()> {
    // Listing sessions needs no CLI overrides — the session directory comes
    // from config files/defaults. `CliArgs` does not implement `Default`,
    // so construct it explicitly with all fields empty.
    let cli_args = CliArgs {
        provider: None,
        api_key: None,
        base_url: None,
        model: None,
        max_tokens: None,
        max_turns: None,
        max_tool_call_malformed_turns: None,
        max_tool_call_failure_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: None,
    };
    let config = Config::resolve(&cli_args)?;
    let session_mgr = SessionManager::new(config.session.directory.clone().into(), config.session.max_sessions);
    let sessions = session_mgr.list()?;
    if sessions.is_empty() {
        eprintln!("No saved sessions.");
    } else {
        eprintln!("{:<8} {:<12} {:<30} {:>5}  Summary", "ID", "Date", "Model", "Msgs");
        for s in &sessions {
            eprintln!(
                "{:<8} {:<12} {:<30} {:>5}  {}",
                s.id,
                s.created_at.format("%Y-%m-%d"),
                s.model,
                s.message_count,
                s.summary
            );
        }
    }
    Ok(())
}
