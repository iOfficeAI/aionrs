use std::env;
use std::sync::Arc;

use aion_agent::engine::AgentEngine;
use aion_agent::error::AgentError;
use aion_agent::output::OutputSink;
use aion_agent::output::terminal::TerminalSink;

use crate::bootstrap::{build_engine, init_logging, resolve_config};
use crate::cli::Cli;
use crate::json_stream;

/// Entry point for the default (non-subcommand) invocation: validates
/// flags, resolves config/logging, then either dispatches to JSON stream
/// mode or bootstraps a terminal engine and runs a single prompt / REPL.
pub(crate) async fn run_main_flow(cli: Cli) -> anyhow::Result<()> {
    if cli.resume.is_some() && cli.session_id.is_some() {
        anyhow::bail!("Cannot use --resume and --session-id together");
    }

    let terminal = Arc::new(TerminalSink::new(cli.no_color));
    let output: Arc<dyn OutputSink> = terminal.clone();

    let config = resolve_config(&cli)?;
    let _log_guard = init_logging(&config, cli.log_dir.as_deref(), cli.log_level.as_deref());

    let cwd = env::current_dir()?.to_string_lossy().to_string();

    // Branch to JSON stream mode
    if cli.json_stream {
        return json_stream::run(config, &cwd, cli.resume, cli.session_id).await;
    }

    let provider_name = config.provider_label.clone();
    let terminal_for_resume = terminal.clone();

    let result = build_engine(config, &cwd, output.clone(), cli.resume.as_deref(), |session| {
        terminal_for_resume.formatter().session_info(&format!(
            "Resumed session {} ({} messages, {} model)",
            session.id,
            session.messages.len(),
            session.model
        ));
    })
    .await?;
    let mut engine = result.engine;

    if cli.resume.is_none() {
        engine.init_session(&provider_name, &cwd, cli.session_id.as_deref())?;
    }

    let prompt = cli.prompt.join(" ");
    if prompt.is_empty() {
        repl_loop(&mut engine, &terminal, &output).await?;
    } else {
        let run_result = engine.run(&prompt, "").await?;
        output.emit_stream_end(
            "",
            run_result.turns,
            run_result.usage.input_tokens,
            run_result.usage.output_tokens,
            run_result.usage.cache_creation_tokens,
            run_result.usage.cache_read_tokens,
        );
    }

    engine.run_stop_hooks().await;

    for mgr in &result.mcp_managers {
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

        if input.is_empty() {
            break;
        }

        match engine.run(input, "").await {
            Ok(result) => {
                if result.turns > 0 {
                    output.emit_stream_end(
                        "",
                        result.turns,
                        result.usage.input_tokens,
                        result.usage.output_tokens,
                        result.usage.cache_creation_tokens,
                        result.usage.cache_read_tokens,
                    );
                }
            }
            Err(AgentError::UserAborted) => break,
            Err(e) => {
                output.emit_error(&e.to_string());
            }
        }
    }

    Ok(())
}
