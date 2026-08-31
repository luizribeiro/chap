mod args;
mod commands;
mod render;
mod tui;

use args::{Cli, resolve_config_path};
use chap_core::AgentBuilder;
use clap::Parser;
use render::{render_load_error, render_session_error, render_start_error};
use std::{env, path::Path, process::ExitCode};

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
    let xdg_config_home = env::var_os("XDG_CONFIG_HOME");
    let home = env::var_os("HOME");
    let config_path = resolve_config_path(
        cli.config,
        Path::new("./chap.json"),
        xdg_config_home.as_deref(),
        home.as_deref(),
    )?;
    let builder = AgentBuilder::load(&config_path).map_err(render_load_error)?;
    match cli.command {
        None => {
            tui::run(builder.start().await.map_err(render_start_error)?).await?;
        }
        Some(command) => command.run(builder).await?,
    }
    Ok(())
}
