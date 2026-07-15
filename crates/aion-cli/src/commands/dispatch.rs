//! Top-level subcommand dispatch for the `aion` CLI binary.

use super::{cmd_auth, cmd_config, cmd_session, cmd_skills};
use crate::cli::Commands;

pub(crate) async fn dispatch(cmd: Commands) -> anyhow::Result<()> {
    match cmd {
        Commands::Config { action } => cmd_config::run(action),
        Commands::Auth { action } => cmd_auth::run(action).await,
        Commands::Session { action } => cmd_session::run(action),
        Commands::Skills { action } => cmd_skills::run(action),
    }
}
