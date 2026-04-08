use std::path::Path;
use std::sync::Arc;

use clap::Parser;

use aionrs::agent;
use aionrs::auth;
use aionrs::config::{self, CliArgs, Config};
use aionrs::context;
use aionrs::engine::AgentEngine;
use aionrs::mcp::manager::McpManager;
use aionrs::mcp::tool_proxy::register_mcp_tools;
use aionrs::output::protocol_sink::ProtocolSink;
use aionrs::output::terminal::TerminalSink;
use aionrs::output::OutputSink;
use aionrs::protocol::{ToolApprovalManager, ToolApprovalResult};
use aionrs::protocol::commands::{ApprovalScope, ProtocolCommand};
use aionrs::protocol::reader::spawn_stdin_reader;
use aionrs::protocol::writer::ProtocolWriter;
use aionrs::provider;
use aionrs::session;
use aionrs::tools::bash::BashTool;
use aionrs::tools::edit::EditTool;
use aionrs::tools::glob::GlobTool;
use aionrs::tools::grep::GrepTool;
use aionrs::tools::read::ReadTool;
use aionrs::tools::registry::ToolRegistry;
use aionrs::tools::spawn::SpawnTool;
use aionrs::tools::write::WriteTool;

#[derive(Parser)]
#[command(name = "aionrs", about = "A multi-provider AI agent CLI with tool orchestration support", version)]
struct Cli {
    /// Provider: "anthropic" or "openai"
    #[arg(short, long, env = "PROVIDER")]
    provider: Option<String>,

    /// API key
    #[arg(short = 'k', long, env = "API_KEY")]
    api_key: Option<String>,

    /// Base URL for the API
    #[arg(short, long, env = "BASE_URL")]
    base_url: Option<String>,

    /// Model name
    #[arg(short, long, env = "MODEL")]
    model: Option<String>,

    /// Max output tokens per response
    #[arg(long)]
    max_tokens: Option<u32>,

    /// Max agent loop turns
    #[arg(long)]
    max_turns: Option<usize>,

    /// Custom system prompt
    #[arg(long)]
    system_prompt: Option<String>,

    /// Named profile from config file
    #[arg(long)]
    profile: Option<String>,

    /// Auto-approve all tool executions (skip confirmation)
    #[arg(long)]
    auto_approve: bool,

    /// Resume a previous session
    #[arg(long)]
    resume: Option<String>,

    /// Use a specific session ID (instead of auto-generating one)
    #[arg(long)]
    session_id: Option<String>,

    /// List saved sessions
    #[arg(long)]
    list_sessions: bool,

    /// Disable colored output
    #[arg(long)]
    no_color: bool,

    /// Enable JSON streaming mode for host client integration
    #[arg(long)]
    json_stream: bool,

    /// Generate a default config file
    #[arg(long)]
    init_config: bool,

    /// Print config file path and exit
    #[arg(long)]
    config_path: bool,

    /// Print skill directory paths and exit
    #[arg(long)]
    skills_path: bool,

    /// Login with Claude.ai account (OAuth device flow)
    #[arg(long)]
    login: bool,

    /// Logout (remove saved OAuth credentials)
    #[arg(long)]
    logout: bool,

    /// Initial prompt (if omitted, enters interactive REPL mode)
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.resume.is_some() && cli.session_id.is_some() {
        anyhow::bail!("Cannot use --resume and --session-id together");
    }

    // Handle --config-path
    if cli.config_path {
        println!("{}", config::global_config_path().display());
        return Ok(());
    }

    // Handle --skills-path
    if cli.skills_path {
        print_skills_paths();
        return Ok(());
    }

    // Handle --init-config
    if cli.init_config {
        return config::init_config();
    }

    // Handle --login / --logout
    if cli.login || cli.logout {
        let oauth = auth::OAuthManager::new(auth::AuthConfig::default());
        if cli.login {
            oauth.login().await?;
            eprintln!("Login successful! You can now use aionrs without --api-key.");
        } else {
            oauth.logout()?;
        }
        return Ok(());
    }

    let terminal = Arc::new(TerminalSink::new(cli.no_color));
    let output: Arc<dyn OutputSink> = terminal.clone();

    // Resolve config from files + CLI args + env vars
    let cli_args = CliArgs {
        provider: cli.provider,
        api_key: cli.api_key,
        base_url: cli.base_url,
        model: cli.model,
        max_tokens: cli.max_tokens,
        max_turns: cli.max_turns,
        system_prompt: cli.system_prompt,
        profile: cli.profile,
        auto_approve: cli.auto_approve,
    };

    let mut config = Config::resolve(&cli_args)?;

    let cwd = std::env::current_dir()?
        .to_string_lossy()
        .to_string();

    // Handle --list-sessions
    if cli.list_sessions {
        let session_mgr = session::SessionManager::new(
            config.session.directory.clone().into(),
            config.session.max_sessions,
        );
        let sessions = session_mgr.list()?;
        if sessions.is_empty() {
            eprintln!("No saved sessions.");
        } else {
            eprintln!("{:<8} {:<12} {:<30} {:>5}  {}", "ID", "Date", "Model", "Msgs", "Summary");
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
        return Ok(());
    }

    // Build system prompt from context
    let system_prompt = context::build_system_prompt(config.system_prompt.as_deref(), &cwd, &[], None);
    config.system_prompt = Some(system_prompt);

    // Register built-in tools
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool));
    registry.register(Box::new(WriteTool));
    registry.register(Box::new(EditTool));
    registry.register(Box::new(BashTool));
    registry.register(Box::new(GrepTool));
    registry.register(Box::new(GlobTool));

    let builtin_names: Vec<String> = registry.tool_names();

    // Connect to MCP servers (if configured)
    let mcp_manager = if !config.mcp.servers.is_empty() {
        match McpManager::connect_all(&config.mcp.servers).await {
            Ok(mgr) => {
                let mgr = Arc::new(mgr);
                register_mcp_tools(&mut registry, &mgr, &builtin_names);
                Some(mgr)
            }
            Err(e) => {
                output.emit_error(&format!("MCP initialization error: {}", e));
                None
            }
        }
    } else {
        None
    };

    // Create provider (shared via Arc for sub-agent reuse)
    let provider = provider::create_provider(&config);

    // Register SpawnTool (sub-agent spawning)
    let spawner = Arc::new(agent::spawner::AgentSpawner::new(
        provider.clone(),
        config.clone(),
    ));
    registry.register(Box::new(SpawnTool::new(spawner)));

    if cli.json_stream {
        return run_json_stream_mode(config, registry, provider, mcp_manager, cli.resume, cli.session_id).await;
    }

    let provider_name = format!("{:?}", config.provider).to_lowercase();

    // Handle --resume
    let mut engine = if let Some(resume_id) = cli.resume {
        let session_mgr = session::SessionManager::new(
            config.session.directory.clone().into(),
            config.session.max_sessions,
        );
        let session = session_mgr.load(&resume_id)?;
        terminal.formatter().session_info(&format!(
            "Resumed session {} ({} messages, {} model)",
            session.id,
            session.messages.len(),
            session.model
        ));
        AgentEngine::resume_with_provider(provider, config, registry, output.clone(), session)
    } else {
        let mut engine = AgentEngine::new_with_provider(provider, config, registry, output.clone());
        engine.init_session(&provider_name, &cwd, cli.session_id.as_deref())?;
        engine
    };

    let prompt = cli.prompt.join(" ");
    if prompt.is_empty() {
        repl_loop(&mut engine, &terminal, &output).await?;
    } else {
        let result = engine.run(&prompt, "").await?;
        output.emit_stream_end(
            "",
            result.turns,
            result.usage.input_tokens,
            result.usage.output_tokens,
            result.usage.cache_creation_tokens,
            result.usage.cache_read_tokens,
        );
    }

    // Run stop hooks before cleanup
    engine.run_stop_hooks().await;

    // Cleanup MCP servers on exit
    if let Some(mgr) = &mcp_manager {
        mgr.shutdown().await;
    }

    Ok(())
}

async fn repl_loop(
    engine: &mut AgentEngine,
    terminal: &Arc<TerminalSink>,
    output: &Arc<dyn OutputSink>,
) -> anyhow::Result<()> {
    use std::io::{self, BufRead};

    loop {
        terminal.formatter().repl_prompt();

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() || input == "/quit" || input == "/exit" {
            break;
        }

        match engine.run(input, "").await {
            Ok(result) => {
                output.emit_stream_end(
                    "",
                    result.turns,
                    result.usage.input_tokens,
                    result.usage.output_tokens,
                    result.usage.cache_creation_tokens,
                    result.usage.cache_read_tokens,
                );
            }
            Err(e) => {
                output.emit_error(&e.to_string());
            }
        }
    }

    Ok(())
}

fn print_skills_paths() {
    use aionrs::skills::paths::{
        project_commands_dirs, project_skills_dirs, user_commands_dir, user_skills_dir,
    };

    fn status(p: &Path) -> &'static str {
        if p.is_dir() { "exists" } else { "not found" }
    }

    // User-level
    match user_skills_dir() {
        Some(dir) => println!("User:    {}  ({})", dir.display(), status(&dir)),
        None => println!("User:    <unable to determine config directory>"),
    }

    // Project-level
    let cwd = std::env::current_dir().unwrap_or_default();
    let project_dirs = project_skills_dirs(&cwd);
    if project_dirs.is_empty() {
        println!("Project: <none found>");
    } else {
        for dir in &project_dirs {
            println!("Project: {}  ({})", dir.display(), status(dir));
        }
    }

    // Legacy commands
    let mut has_legacy = false;
    if let Some(dir) = user_commands_dir() {
        if dir.is_dir() {
            println!("Legacy:  {}  ({})", dir.display(), status(&dir));
            has_legacy = true;
        }
    }
    for dir in project_commands_dirs(&cwd) {
        println!("Legacy:  {}  ({})", dir.display(), status(&dir));
        has_legacy = true;
    }
    if !has_legacy {
        println!("Legacy:  <none found>");
    }
}

async fn run_json_stream_mode(
    config: Config,
    registry: ToolRegistry,
    provider: Arc<dyn aionrs::provider::LlmProvider>,
    mcp_manager: Option<Arc<McpManager>>,
    resume: Option<String>,
    session_id: Option<String>,
) -> anyhow::Result<()> {
    let writer = Arc::new(ProtocolWriter::new());
    let protocol_sink = Arc::new(ProtocolSink::new(writer.clone()));
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let output: Arc<dyn OutputSink> = protocol_sink.clone();
    let has_mcp = mcp_manager.is_some();

    let provider_name = format!("{:?}", config.provider).to_lowercase();
    let cwd = std::env::current_dir()?.to_string_lossy().to_string();

    let mut engine = if let Some(resume_id) = resume {
        let session_mgr = session::SessionManager::new(
            config.session.directory.clone().into(),
            config.session.max_sessions,
        );
        let session = session_mgr.load(&resume_id)?;
        AgentEngine::resume_with_provider(provider, config, registry, output.clone(), session)
    } else {
        let mut engine = AgentEngine::new_with_provider(provider, config, registry, output.clone());
        engine.init_session(&provider_name, &cwd, session_id.as_deref())?;
        engine
    };

    let sid = engine.current_session_id();
    protocol_sink.emit_ready(has_mcp, sid);

    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer.clone());

    let mut cmd_rx = spawn_stdin_reader();

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            ProtocolCommand::Message { msg_id, input, files: _ } => {
                // Run engine in a select! loop so we can process approval/deny
                // commands while the engine awaits tool approval.
                let engine_fut = engine.run(&input, &msg_id);
                tokio::pin!(engine_fut);

                let mut stopped = false;
                loop {
                    tokio::select! {
                        result = &mut engine_fut => {
                            match result {
                                Ok(result) => {
                                    output.emit_stream_end(
                                        &msg_id,
                                        result.turns,
                                        result.usage.input_tokens,
                                        result.usage.output_tokens,
                                        result.usage.cache_creation_tokens,
                                        result.usage.cache_read_tokens,
                                    );
                                }
                                Err(e) => {
                                    output.emit_error(&e.to_string());
                                }
                            }
                            break;
                        }
                        Some(sub_cmd) = cmd_rx.recv() => {
                            match sub_cmd {
                                ProtocolCommand::ToolApprove { call_id, scope } => {
                                    if matches!(scope, ApprovalScope::Always) {
                                        // Auto-approve future calls of this category
                                    }
                                    approval_manager.resolve(&call_id, ToolApprovalResult::Approved);
                                }
                                ProtocolCommand::ToolDeny { call_id, reason } => {
                                    approval_manager.resolve(&call_id, ToolApprovalResult::Denied { reason });
                                }
                                ProtocolCommand::Stop => {
                                    stopped = true;
                                    break;
                                }
                                _ => {
                                    eprintln!("[protocol] Ignoring command during active message processing");
                                }
                            }
                        }
                    }
                }
                if stopped {
                    break;
                }
            }
            ProtocolCommand::Stop => {
                break;
            }
            ProtocolCommand::ToolApprove { call_id, scope } => {
                if matches!(scope, ApprovalScope::Always) {
                    // Auto-approve all future calls of this tool's category
                }
                approval_manager.resolve(&call_id, ToolApprovalResult::Approved);
            }
            ProtocolCommand::ToolDeny { call_id, reason } => {
                approval_manager.resolve(&call_id, ToolApprovalResult::Denied { reason });
            }
            ProtocolCommand::InitHistory { text } => {
                eprintln!("[protocol] InitHistory received: {} chars", text.len());
            }
            ProtocolCommand::SetMode { mode: _ } => {
                eprintln!("[protocol] SetMode received");
            }
        }
    }

    engine.run_stop_hooks().await;
    if let Some(mgr) = &mcp_manager {
        mgr.shutdown().await;
    }

    Ok(())
}
