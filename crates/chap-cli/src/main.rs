mod args;
mod commands;
mod render;
mod tui;

use args::{Cli, Command, Grants, GrantsCommand, resolve_config_path};
use chap_core::AgentBuilder;
use clap::Parser;
use commands::grants::grants_review;
use render::{render_consent_error, render_load_error, render_session_error, render_start_error};
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
        Some(Command::Plugins(plugins)) => plugins.command.run(builder).await?,
        Some(Command::Grants(Grants {
            command: GrantsCommand::Review { instance_id },
        })) => print!("{}", grants_review(&builder, instance_id.as_deref()).await?),
        Some(Command::Grants(Grants {
            command: GrantsCommand::Approve { instance_id },
        })) => {
            builder
                .approve_plugin(&instance_id)
                .await
                .map_err(render_consent_error)?;
            println!(
                "Approved `{instance_id}` for its exact resolved manifest. Concrete scopes remain configured in chap.json."
            );
        }
        Some(Command::Grants(Grants {
            command: GrantsCommand::Deny { instance_id },
        })) => {
            builder
                .deny_plugin(&instance_id)
                .map_err(render_consent_error)?;
            println!("Denied `{instance_id}`. It will require approval before its next admission.");
        }
    }
    Ok(())
}
