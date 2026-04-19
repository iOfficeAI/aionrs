use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::Parser;

use aion_agent::context;
use aion_agent::engine::AgentEngine;
use aion_agent::output::OutputSink;
use aion_agent::output::protocol_sink::ProtocolSink;
use aion_agent::output::terminal::TerminalSink;
use aion_agent::plan::tools::{EnterPlanModeTool, ExitPlanModeTool};
use aion_agent::session;
use aion_agent::skill_tool::SkillTool;
use aion_agent::spawn_tool::SpawnTool;
use aion_agent::spawner::AgentSpawner;
use aion_config::auth;
use aion_config::config::{self, CliArgs, Config, McpServerConfig, TransportType};
use aion_mcp::manager::McpManager;
use aion_mcp::tool_proxy::{register_mcp_tools, register_single_server_tools};
use aion_protocol::commands::{ApprovalScope, ProtocolCommand};
use aion_protocol::events::ProtocolEvent;
use aion_protocol::reader::spawn_stdin_reader;
use aion_protocol::writer::ProtocolWriter;
use aion_protocol::{ToolApprovalManager, ToolApprovalResult};
use aion_skills::loader::load_all_skills;
use aion_skills::permissions::SkillPermissionChecker;
use aion_tools::bash::BashTool;
use aion_tools::edit::EditTool;
use aion_tools::file_cache::FileStateCache;
use aion_tools::glob::GlobTool;
use aion_tools::grep::GrepTool;
use aion_tools::read::ReadTool;
use aion_tools::registry::ToolRegistry;
use aion_tools::tool_search::ToolSearchTool;
use aion_tools::write::WriteTool;

#[derive(Parser)]
#[command(
    name = "aionrs",
    about = "A multi-provider AI agent CLI with tool orchestration support",
    version
)]
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

    /// Login with Anthropic account (OAuth device flow)
    #[arg(long)]
    login: bool,

    /// Logout (remove saved OAuth credentials)
    #[arg(long)]
    logout: bool,

    /// Output compaction level: off, safe (default), full
    #[arg(long)]
    compaction: Option<String>,

    /// Enable TOON encoding for JSON arrays (session-level, cannot change mid-conversation)
    #[arg(long)]
    toon: bool,

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

    if let Some(ref level_str) = cli.compaction {
        match level_str.parse::<aion_compact::CompactionLevel>() {
            Ok(level) => config.compact.compaction = level,
            Err(e) => anyhow::bail!("Invalid --compaction value: {e}"),
        }
    }
    if cli.toon {
        config.compact.toon = true;
    }

    let cwd = std::env::current_dir()?.to_string_lossy().to_string();

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
            eprintln!(
                "{:<8} {:<12} {:<30} {:>5}  Summary",
                "ID", "Date", "Model", "Msgs"
            );
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

    // Resolve memory directory for the current project
    let cwd_path = std::path::Path::new(&cwd);
    let memory_dir = aion_memory::paths::auto_memory_dir(cwd_path);

    // Create file state cache (shared across Read/Edit/Write tools)
    let file_cache = if config.file_cache.enabled {
        Some(Arc::new(std::sync::RwLock::new(FileStateCache::new(
            &config.file_cache,
        ))))
    } else {
        None
    };

    // Register built-in tools
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(file_cache.clone())));
    registry.register(Box::new(WriteTool::new(file_cache.clone())));
    registry.register(Box::new(EditTool::new(file_cache)));
    registry.register(Box::new(BashTool));
    registry.register(Box::new(GrepTool));
    registry.register(Box::new(GlobTool));

    let builtin_names: Vec<String> = registry.tool_names();

    // Connect to MCP servers (if configured)
    let mcp_manager = if !config.mcp.servers.is_empty() {
        match McpManager::connect_all(&config.mcp.servers).await {
            Ok(mgr) => {
                let mgr = Arc::new(mgr);
                register_mcp_tools(&mut registry, &mgr, &builtin_names, &config.mcp.servers);
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

    // Load skills from all sources (bundled, MCP, user, project)
    let skills = load_all_skills(cwd_path, &[], false, mcp_manager.as_deref()).await;

    // Build system prompt with loaded skills
    let mut prompt_cache = aion_agent::context::SystemPromptCache::new();
    let system_prompt = context::build_system_prompt(
        &mut prompt_cache,
        config.system_prompt.as_deref(),
        &cwd,
        &config.model,
        &skills,
        None,
        memory_dir.as_deref(),
        false,
        config.compact.toon,
    );
    config.system_prompt = Some(system_prompt);

    // Register SkillTool so the LLM can invoke skills
    let skills_arc = Arc::new(skills);
    let skill_checker = SkillPermissionChecker::new(
        config.tools.skills.deny.clone(),
        config.tools.skills.allow.clone(),
        config.tools.auto_approve,
    );
    registry.register(Box::new(SkillTool::new(
        skills_arc,
        cwd.clone(),
        skill_checker,
    )));

    // Create provider (shared via Arc for sub-agent reuse)
    let provider = aion_providers::create_provider(&config);

    // Register SpawnTool (sub-agent spawning)
    let spawner = Arc::new(AgentSpawner::new(provider.clone(), config.clone()));
    registry.register(Box::new(SpawnTool::new(spawner.clone())));

    // Register Plan Mode tools (if enabled)
    let plan_active_flag = Arc::new(AtomicBool::new(false));
    if config.plan.enabled {
        registry.register(Box::new(EnterPlanModeTool::new(Arc::clone(
            &plan_active_flag,
        ))));
        registry.register(Box::new(ExitPlanModeTool::new(Arc::clone(
            &plan_active_flag,
        ))));
    }

    // Register ToolSearch (must be after all other tools to capture full snapshot)
    let tool_defs_snapshot = registry.to_tool_defs();
    registry.register(Box::new(ToolSearchTool::new(tool_defs_snapshot)));

    if cli.json_stream {
        return run_json_stream_mode(
            config,
            registry,
            provider,
            mcp_manager,
            cli.resume,
            cli.session_id,
            plan_active_flag.clone(),
        )
        .await;
    }

    let provider_name = config.provider_label.clone();

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
    engine.set_plan_active_flag(plan_active_flag.clone());

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
    use aion_skills::paths::{
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
    if let Some(dir) = user_commands_dir()
        && dir.is_dir()
    {
        println!("Legacy:  {}  ({})", dir.display(), status(&dir));
        has_legacy = true;
    }
    for dir in project_commands_dirs(&cwd) {
        println!("Legacy:  {}  ({})", dir.display(), status(&dir));
        has_legacy = true;
    }
    if !has_legacy {
        println!("Legacy:  <none found>");
    }
}

fn to_mcp_server_config(
    transport: &str,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    url: Option<String>,
    headers: Option<HashMap<String, String>>,
) -> Result<McpServerConfig, String> {
    let transport_type = match transport {
        "stdio" => TransportType::Stdio,
        "sse" => TransportType::Sse,
        "streamable-http" | "streamable_http" => TransportType::StreamableHttp,
        other => return Err(format!("unknown transport: {other}")),
    };
    Ok(McpServerConfig {
        transport: transport_type,
        command,
        args,
        env,
        url,
        headers,
        deferred: Some(false),
    })
}

/// Pending config fields: (model, thinking, thinking_budget, effort)
type PendingConfig = (
    Option<String>,
    Option<String>,
    Option<u32>,
    Option<String>,
    Option<String>,
);

async fn run_json_stream_mode(
    config: Config,
    registry: ToolRegistry,
    provider: Arc<dyn aion_providers::LlmProvider>,
    mcp_manager: Option<Arc<McpManager>>,
    resume: Option<String>,
    session_id: Option<String>,
    plan_active_flag: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let writer = Arc::new(ProtocolWriter::new());
    let protocol_sink = Arc::new(ProtocolSink::new(writer.clone()));
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let output: Arc<dyn OutputSink> = protocol_sink.clone();
    let initial_has_mcp = mcp_manager.is_some();

    let provider_name = config.provider_label.clone();
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
    engine.set_plan_active_flag(plan_active_flag);

    let sid = engine.current_session_id();
    protocol_sink.emit_ready(
        engine.compat(),
        initial_has_mcp,
        sid,
        &approval_manager.current_mode(),
    );

    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer.clone());

    let mut cmd_rx = spawn_stdin_reader();

    // --- Pre-message phase: accept AddMcpServer commands ---
    // Dynamic servers get their own McpManager instances because the initial
    // mcp_manager Arc may already be shared by tool proxies (Arc::get_mut fails).
    let mut dynamic_managers: Vec<Arc<McpManager>> = Vec::new();
    let mut first_cmd: Option<ProtocolCommand> = None;

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            ProtocolCommand::AddMcpServer {
                name,
                transport,
                command,
                args,
                env,
                url,
                headers,
            } => {
                eprintln!(
                    "[mcp] AddMcpServer received: name={name}, transport={transport}, command={command:?}"
                );
                let config =
                    match to_mcp_server_config(&transport, command, args, env, url, headers) {
                        Ok(c) => c,
                        Err(e) => {
                            output.emit_error(&format!("AddMcpServer '{name}': {e}"));
                            continue;
                        }
                    };

                let mut single_configs = HashMap::new();
                single_configs.insert(name.clone(), config.clone());
                eprintln!("[mcp] Connecting to '{name}'...");
                match McpManager::connect_all(&single_configs).await {
                    Ok(mgr) => {
                        let tool_names: Vec<String> = mgr
                            .all_tools()
                            .iter()
                            .map(|(_, t)| t.name.clone())
                            .collect();
                        eprintln!("[mcp] Connected to '{name}': {} tools", tool_names.len());
                        let mgr_arc = Arc::new(mgr);
                        let builtin_names = engine.tool_names();
                        register_single_server_tools(
                            engine.registry_mut(),
                            &mgr_arc,
                            &name,
                            &builtin_names,
                            config.deferred.unwrap_or(true),
                        );
                        dynamic_managers.push(mgr_arc);
                        let _ = writer.emit(&ProtocolEvent::McpReady {
                            name,
                            tools: tool_names,
                        });
                    }
                    Err(e) => {
                        eprintln!("[mcp] connect_one failed for '{name}': {e}");
                        output.emit_error(&format!("AddMcpServer '{name}' failed: {e}"));
                    }
                }
            }
            ProtocolCommand::Stop => return Ok(()),
            other => {
                first_cmd = Some(other);
                break;
            }
        }
    }

    let has_mcp = mcp_manager.is_some() || !dynamic_managers.is_empty();
    let mut pending_cmd = first_cmd;

    loop {
        let cmd = if let Some(c) = pending_cmd.take() {
            c
        } else {
            match cmd_rx.recv().await {
                Some(c) => c,
                None => break,
            }
        };

        match cmd {
            ProtocolCommand::Message {
                msg_id,
                content,
                files: _,
            } => {
                let mut stopped = false;
                let mut pending_config: Option<PendingConfig> = None;
                let mut mode_changed = false;

                {
                    let engine_fut = engine.run(&content, &msg_id);
                    tokio::pin!(engine_fut);

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
                                        // Always emit stream_end so the client
                                        // can leave the "running" state.
                                        output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0);
                                    }
                                }
                                break;
                            }
                            Some(sub_cmd) = cmd_rx.recv() => {
                                match sub_cmd {
                                    ProtocolCommand::ToolApprove { call_id, scope: _ } => {
                                        approval_manager.resolve(&call_id, ToolApprovalResult::Approved);
                                    }
                                    ProtocolCommand::ToolDeny { call_id, reason } => {
                                        approval_manager.resolve(&call_id, ToolApprovalResult::Denied { reason });
                                    }
                                    ProtocolCommand::Stop => {
                                        stopped = true;
                                        break;
                                    }
                                    ProtocolCommand::SetConfig { model, thinking, thinking_budget, effort, compaction } => {
                                        pending_config = Some((model, thinking, thinking_budget, effort, compaction));
                                        let _ = writer.emit(&aion_protocol::events::ProtocolEvent::Info {
                                            msg_id: String::new(),
                                            message: "set_config: queued, will apply after current response".to_string(),
                                        });
                                    }
                                    ProtocolCommand::SetMode { mode } => {
                                        approval_manager.set_mode(mode);
                                        mode_changed = true;
                                        let _ = writer.emit(&aion_protocol::events::ProtocolEvent::Info {
                                            msg_id: String::new(),
                                            message: format!("mode updated: {}", approval_manager.current_mode()),
                                        });
                                    }
                                    _ => {
                                        eprintln!("[protocol] Ignoring command during active message processing");
                                    }
                                }
                            }
                        }
                    }
                } // engine_fut dropped here, releasing mutable borrow

                // Apply any config changes that arrived during processing
                if let Some((model, thinking, thinking_budget, effort, compaction)) =
                    pending_config.take()
                {
                    let changes = engine.apply_config_update(
                        model,
                        thinking,
                        thinking_budget,
                        effort,
                        compaction,
                    );
                    if !changes.is_empty() {
                        let _ = writer.emit(&aion_protocol::events::ProtocolEvent::Info {
                            msg_id: String::new(),
                            message: format!("config applied: {}", changes.join(", ")),
                        });
                    }
                    // config_changed covers both config and mode updates
                    protocol_sink.emit_config_changed(
                        engine.compat(),
                        has_mcp,
                        &approval_manager.current_mode(),
                    );
                } else if mode_changed {
                    protocol_sink.emit_config_changed(
                        engine.compat(),
                        has_mcp,
                        &approval_manager.current_mode(),
                    );
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
            ProtocolCommand::SetMode { mode } => {
                let mode_str = format!("{mode:?}").to_lowercase();
                approval_manager.set_mode(mode);
                let _ = writer.emit(&aion_protocol::events::ProtocolEvent::Info {
                    msg_id: String::new(),
                    message: format!("mode updated: {}", approval_manager.current_mode()),
                });
                protocol_sink.emit_config_changed(
                    engine.compat(),
                    has_mcp,
                    &approval_manager.current_mode(),
                );
                eprintln!("[protocol] SetMode applied: {mode_str}");
            }
            ProtocolCommand::SetConfig {
                model,
                thinking,
                thinking_budget,
                effort,
                compaction,
            } => {
                let changes = engine.apply_config_update(
                    model,
                    thinking,
                    thinking_budget,
                    effort,
                    compaction,
                );
                let message = if changes.is_empty() {
                    "set_config: no changes".to_string()
                } else {
                    format!("config updated: {}", changes.join(", "))
                };
                let _ = writer.emit(&aion_protocol::events::ProtocolEvent::Info {
                    msg_id: String::new(),
                    message,
                });
                protocol_sink.emit_config_changed(
                    engine.compat(),
                    has_mcp,
                    &approval_manager.current_mode(),
                );
            }
            ProtocolCommand::AddMcpServer { name, .. } => {
                output.emit_error(&format!(
                    "AddMcpServer '{name}': rejected — only allowed before first Message"
                ));
            }
        }
    }

    engine.run_stop_hooks().await;
    if let Some(mgr) = &mcp_manager {
        mgr.shutdown().await;
    }
    for mgr in &dynamic_managers {
        mgr.shutdown().await;
    }

    Ok(())
}
