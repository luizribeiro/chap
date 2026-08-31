mod check;
mod list;

use chap_core::AgentBuilder;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub(crate) struct Plugins {
    #[command(subcommand)]
    pub(crate) command: PluginsCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PluginsCommand {
    /// Check that configured plugins can be loaded.
    Check,
    /// List configured plugins.
    List,
}

impl PluginsCommand {
    pub(crate) async fn run(self, builder: AgentBuilder) -> Result<(), String> {
        match self {
            Self::Check => check::plugin_check(builder).await,
            Self::List => {
                print!("{}", list::plugin_list(&builder)?);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{Cli, Command};
    use clap::Parser;

    #[test]
    fn parses_plugin_commands() {
        let cli = Cli::try_parse_from(["chap", "plugins", "list"]).unwrap();

        assert!(matches!(
            cli.command,
            Some(Command::Plugins(Plugins {
                command: PluginsCommand::List
            }))
        ));
    }
}
