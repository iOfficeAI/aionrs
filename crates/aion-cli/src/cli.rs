use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "aionrs",
    about = "A multi-provider AI agent CLI with tool orchestration support",
    version
)]
pub(crate) struct Cli {
    // --- Subcommand ---
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    // --- Provider / model connection ---
    /// Provider: "anthropic" or "openai"
    #[arg(short, long, env = "PROVIDER")]
    pub(crate) provider: Option<String>,

    /// API key
    #[arg(short = 'k', long, env = "API_KEY")]
    pub(crate) api_key: Option<String>,

    /// Base URL for the API
    #[arg(short, long, env = "BASE_URL")]
    pub(crate) base_url: Option<String>,

    /// Model name
    #[arg(short, long, env = "MODEL")]
    pub(crate) model: Option<String>,

    /// Max output tokens per response
    #[arg(long)]
    pub(crate) max_tokens: Option<u32>,

    // --- Runtime guards ---
    /// Max model turns per run. Defaults to 20; 0 disables.
    #[arg(long)]
    pub(crate) max_turns: Option<usize>,

    /// Max consecutive same tool-call-malformed rounds before stopping. 0 disables.
    #[arg(long)]
    pub(crate) max_tool_call_malformed_turns: Option<usize>,

    /// Max consecutive tool-call-failure rounds before stopping. 0 disables.
    #[arg(long)]
    pub(crate) max_tool_call_failure_turns: Option<usize>,

    // --- Prompt / profile ---
    /// Custom system prompt
    #[arg(long)]
    pub(crate) system_prompt: Option<String>,

    /// Named profile from config file
    #[arg(long)]
    pub(crate) profile: Option<String>,

    /// Auto-approve all tool executions (skip confirmation)
    #[arg(long)]
    pub(crate) auto_approve: bool,

    /// Project directory to load .aionrs.toml from (defaults to CWD)
    #[arg(long)]
    pub(crate) project_dir: Option<PathBuf>,

    // --- Session ---
    /// Resume a previous session
    #[arg(long)]
    pub(crate) resume: Option<String>,

    /// Use a specific session ID (instead of auto-generating one)
    #[arg(long)]
    pub(crate) session_id: Option<String>,

    // --- Output ---
    /// Disable colored output
    #[arg(long)]
    pub(crate) no_color: bool,

    /// Enable JSON streaming mode for host client integration
    #[arg(long)]
    pub(crate) json_stream: bool,

    /// Output compaction level: off, safe (default), full
    #[arg(long)]
    pub(crate) compaction: Option<String>,

    /// Enable TOON encoding for JSON arrays (session-level, cannot change mid-conversation)
    #[arg(long)]
    pub(crate) toon: bool,

    // --- Logging ---
    /// Log directory (enables file logging)
    #[arg(long)]
    pub(crate) log_dir: Option<String>,

    /// Log level filter (e.g. "info", "debug", "info,aion_providers=debug")
    #[arg(long)]
    pub(crate) log_level: Option<String>,

    // --- Trailing prompt ---
    /// Initial prompt (if omitted, enters interactive REPL mode)
    #[arg(trailing_var_arg = true)]
    pub(crate) prompt: Vec<String>,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Configuration file management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Authentication (Anthropic OAuth)
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Session management
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Skills directory introspection
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
    },
}

#[derive(Subcommand)]
pub(crate) enum ConfigAction {
    /// Generate a default config file
    Init,
    /// Print config file path and exit
    Path,
}

#[derive(Subcommand)]
pub(crate) enum AuthAction {
    /// Login with Anthropic account (OAuth device flow)
    Login,
    /// Logout (remove saved OAuth credentials)
    Logout,
}

#[derive(Subcommand)]
pub(crate) enum SessionAction {
    /// List saved sessions
    List,
}

#[derive(Subcommand)]
pub(crate) enum SkillsAction {
    /// Print skill directory paths and exit
    Path,
}

#[cfg(test)]
#[path = "cli_test.rs"]
mod cli_test;
