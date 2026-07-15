use aion_config::config;

use crate::cli::ConfigAction;

pub(crate) fn run(action: ConfigAction) -> anyhow::Result<()> {
    match action {
        ConfigAction::Init => config::init_config(),
        ConfigAction::Path => {
            println!("{}", config::global_config_path().display());
            Ok(())
        }
    }
}
