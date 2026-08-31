mod args;
mod commands;
mod render;
mod tui;

use args::{Cli, Command, Grants, GrantsCommand, Plugins, PluginsCommand, resolve_config_path};
use chap_core::AgentBuilder;
use clap::Parser;
use commands::{NO_PLUGINS_CONFIGURED, grants::grants_review};
use render::{render_consent_error, render_load_error, render_session_error, render_start_error};
use std::{env, path::Path, process::ExitCode};
use unicode_width::UnicodeWidthStr;

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

fn plugin_list(builder: &AgentBuilder) -> Result<String, String> {
    let mut rows = Vec::new();
    for (id, component) in builder.plugins() {
        let roles = builder.plugin_roles(id).map_err(render_consent_error)?;
        rows.push([
            id.to_owned(),
            if roles.is_empty() {
                "-".to_owned()
            } else {
                roles.join(", ")
            },
            component.to_string_lossy().into_owned(),
        ]);
    }
    if rows.is_empty() {
        return Ok(NO_PLUGINS_CONFIGURED.to_owned());
    }
    Ok(render_table(&rows))
}

fn render_table(rows: &[[String; 3]]) -> String {
    const HEADERS: [&str; 3] = ["ID", "ROLES", "COMPONENT"];

    let mut widths = HEADERS.map(UnicodeWidthStr::width);
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(UnicodeWidthStr::width(cell.as_str()));
        }
    }

    let mut output = String::new();
    push_row(&mut output, HEADERS, widths);
    for row in rows {
        push_row(&mut output, row.each_ref().map(String::as_str), widths);
    }
    output
}

fn push_row(output: &mut String, row: [&str; 3], widths: [usize; 3]) {
    for (index, cell) in row.into_iter().enumerate() {
        output.push_str(cell);
        if index < row.len() - 1 {
            output.push_str(&" ".repeat(widths[index] - UnicodeWidthStr::width(cell) + 2));
        }
    }
    output.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aligns_plugin_table_columns() {
        let rows = [
            ["short".into(), "provider".into(), "one.wasm".into()],
            ["much-longer".into(), "-".into(), "two.wasm".into()],
        ];

        assert_eq!(
            render_table(&rows),
            "ID           ROLES     COMPONENT\n\
             short        provider  one.wasm\n\
             much-longer  -         two.wasm\n"
        );
    }

    #[test]
    fn describes_an_empty_plugin_list() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("chap.json");
        std::fs::write(&config_path, "{}").unwrap();
        let builder = AgentBuilder::load(&config_path).unwrap();

        assert_eq!(
            plugin_list(&builder).unwrap(),
            "No plugins are configured.\n"
        );
    }
}
