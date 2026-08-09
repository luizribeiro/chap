use clap::{Args, Parser, Subcommand};
use std::{path::PathBuf, process::ExitCode};

mod application;
mod config;

#[derive(Debug, Parser)]
#[command(version, about = "A plugin-powered coding agent")]
struct Cli {
    /// Configuration file to read.
    #[arg(long, default_value = "sage.toml", global = true)]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
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

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let config = config::Config::load(&cli.config)?;
    match cli.command {
        Command::Plugins(Plugins {
            command: PluginsCommand::Check,
        }) => {
            let application = application::Application::load(&config)?;
            println!("Loaded {} plugin(s).", application.plugin_count());
        }
        Command::Plugins(Plugins {
            command: PluginsCommand::List,
        }) => print!("{}", plugin_list(&config)),
    }
    Ok(())
}

fn plugin_list(config: &config::Config) -> String {
    let mut output = String::from("ID\tCOMPONENT\n");
    for (id, plugin) in config.plugins() {
        output.push_str(id);
        output.push('\t');
        output.push_str(&plugin.component().to_string_lossy());
        output.push('\n');
    }
    output
}
