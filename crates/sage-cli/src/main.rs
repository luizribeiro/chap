mod tui;

use clap::{Args, Parser, Subcommand};
use sage_core::SageBuilder;
use std::{path::PathBuf, process::ExitCode};

#[derive(Debug, Parser)]
#[command(version, about = "A plugin-powered coding agent")]
struct Cli {
    /// Configuration file to read.
    #[arg(long, default_value = "sage.toml", global = true)]
    config: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect configured plugins.
    Plugins(Plugins),
}

#[derive(Debug, Args)]
struct Plugins {
    #[command(subcommand)]
    command: PluginsCommand,
}

#[derive(Debug, Subcommand)]
enum PluginsCommand {
    /// Check that configured plugins can be loaded.
    Check,
    /// List configured plugins.
    List,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let builder = SageBuilder::load(&cli.config)?;
    match cli.command {
        None => {
            tui::run(builder.start().await?).await?;
        }
        Some(Command::Plugins(Plugins {
            command: PluginsCommand::Check,
        })) => {
            let plugin_count = builder.plugins().count();
            builder.start().await?;
            println!("Loaded {plugin_count} plugin(s).");
        }
        Some(Command::Plugins(Plugins {
            command: PluginsCommand::List,
        })) => print!("{}", plugin_list(&builder)),
    }
    Ok(())
}

fn plugin_list(builder: &SageBuilder) -> String {
    let mut output = String::from("ID\tCOMPONENT\n");
    for (id, component) in builder.plugins() {
        output.push_str(id);
        output.push('\t');
        output.push_str(&component.to_string_lossy());
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_the_tui_when_no_subcommand_is_given() {
        let cli = Cli::try_parse_from(["sage"]).unwrap();

        assert!(cli.command.is_none());
    }

    #[test]
    fn still_parses_plugin_commands() {
        let cli = Cli::try_parse_from(["sage", "plugins", "list"]).unwrap();

        assert!(matches!(
            cli.command,
            Some(Command::Plugins(Plugins {
                command: PluginsCommand::List
            }))
        ));
    }
}
