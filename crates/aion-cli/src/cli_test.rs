use clap::{CommandFactory, Parser};

use super::{Cli, Commands, ConfigAction};

#[test]
fn cli_definition_is_valid() {
    Cli::command().debug_assert();
}

#[test]
fn no_subcommand_parses_prompt_as_trailing_args() {
    let cli = Cli::try_parse_from(["aionrs", "write", "a", "function"]).unwrap();
    assert!(cli.command.is_none());
    assert_eq!(cli.prompt, vec!["write", "a", "function"]);
}

#[test]
fn config_init_parses_to_config_action() {
    let cli = Cli::try_parse_from(["aionrs", "config", "init"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Config {
            action: ConfigAction::Init
        })
    ));
}

#[test]
fn deleted_flags_are_rejected() {
    assert!(Cli::try_parse_from(["aionrs", "--config-path"]).is_err());
    assert!(Cli::try_parse_from(["aionrs", "--login"]).is_err());
    assert!(Cli::try_parse_from(["aionrs", "--list-sessions"]).is_err());
    assert!(Cli::try_parse_from(["aionrs", "--skills-path"]).is_err());
    assert!(Cli::try_parse_from(["aionrs", "--init-config"]).is_err());
    assert!(Cli::try_parse_from(["aionrs", "--logout"]).is_err());
}
