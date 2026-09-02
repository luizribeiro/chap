mod args;
mod commands;
mod render;
mod telemetry;
mod tui;

use args::{Cli, resolve_config_path};
use chap_core::AgentBuilder;
use clap::Parser;
use render::{render_load_error, render_session_error, render_start_error};
use std::{env, path::Path, process::ExitCode};

fn main() -> ExitCode {
    let tracing = telemetry::TracingRouter::new();
    telemetry::install(tracing.clone());
    chap_core::install_crypto_provider();
    async_main(tracing)
}

#[tokio::main]
async fn async_main(tracing: telemetry::TracingRouter) -> ExitCode {
    match run(Cli::parse(), &tracing).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli, tracing: &telemetry::TracingRouter) -> Result<(), String> {
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
            let consent_path = builder.consent_path().map_err(render_load_error)?;
            tui::run(
                builder.start().await.map_err(render_start_error)?,
                &consent_path,
                tracing,
            )
            .await?;
        }
        Some(command) => command.run(builder).await?,
    }
    Ok(())
}
