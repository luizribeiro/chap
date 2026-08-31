mod args;
mod commands;
mod render;
mod tui;

use args::{Cli, Command, Grants, GrantsCommand, Plugins, PluginsCommand, resolve_config_path};
use chap_core::AgentBuilder;
use clap::Parser;
use commands::{grants::grants_review, plugins::plugin_list};
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
        Some(Command::Plugins(Plugins {
            command: PluginsCommand::Check,
        })) => {
            let plugin_count = builder.plugins().count();
            let agent = builder.start().await.map_err(render_start_error)?;
            tokio::task::spawn_blocking(move || drop(agent))
                .await
                .map_err(|error| format!("failed to clean up plugin host: {error}"))?;
            println!("Loaded {plugin_count} plugin(s).");
        }
        Some(Command::Plugins(Plugins {
            command: PluginsCommand::List,
        })) => print!("{}", plugin_list(&builder)?),
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
