use clap::Parser;

mod bootstrap;
mod cli;
mod commands;
mod json_stream;
mod run;

use cli::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut cli = Cli::parse();
    let command = cli.command.take();
    match command {
        Some(cmd) => commands::dispatch(cmd).await,
        None => run::run_main_flow(cli).await,
    }
}
