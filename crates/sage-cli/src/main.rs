mod tui;

use clap::{Args, Parser, Subcommand};
use sage_core::{AgentBuilder, DriftChange, DriftKind, PluginConsentReview};
use std::{collections::BTreeSet, path::PathBuf, process::ExitCode};
use unicode_width::UnicodeWidthStr;

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
    /// Review and manage plugin permission grants.
    Grants(Grants),
    /// Inspect configured plugins.
    Plugins(Plugins),
}

#[derive(Debug, Args)]
struct Grants {
    #[command(subcommand)]
    command: GrantsCommand,
}

#[derive(Debug, Subcommand)]
enum GrantsCommand {
    /// Review resolved permission requests without approving them.
    Review {
        /// Plugin instance to review; omit to review every configured plugin.
        instance_id: Option<String>,
    },
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
    let builder = AgentBuilder::load(&cli.config)?;
    match cli.command {
        None => {
            tui::run(builder.start().await?).await?;
        }
        Some(Command::Plugins(Plugins {
            command: PluginsCommand::Check,
        })) => {
            let plugin_count = builder.plugins().count();
            let agent = builder.start().await?;
            let errors = agent
                .plugin_errors()
                .map(|(_, error)| error.to_owned())
                .collect::<Vec<_>>();
            tokio::task::spawn_blocking(move || drop(agent))
                .await
                .map_err(|error| format!("failed to clean up plugin host: {error}"))?;
            if !errors.is_empty() {
                return Err(errors.join("\n"));
            }
            println!("Loaded {plugin_count} plugin(s).");
        }
        Some(Command::Plugins(Plugins {
            command: PluginsCommand::List,
        })) => print!("{}", plugin_list(&builder)?),
        Some(Command::Grants(Grants {
            command: GrantsCommand::Review { instance_id },
        })) => print!("{}", grants_review(&builder, instance_id.as_deref()).await?),
    }
    Ok(())
}

async fn grants_review(
    builder: &AgentBuilder,
    instance_id: Option<&str>,
) -> Result<String, String> {
    let ids = match instance_id {
        Some(id) => vec![id.to_owned()],
        None => builder
            .plugins()
            .map(|(id, _)| id.to_owned())
            .collect::<Vec<_>>(),
    };
    if ids.is_empty() {
        return Ok("No plugins are configured.\n".to_owned());
    }

    let mut output = String::from(
        "Concrete scopes come from sage.toml; approval grants this exact resolved manifest.\n",
    );
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        output.push_str(&render_grant_review(&builder.review_plugin(id).await?));
    }
    Ok(output)
}

fn render_grant_review(review: &PluginConsentReview) -> String {
    let mut output = format!(
        "Instance: {}\nPlugin: {}\n",
        review.manifest.instance_id, review.manifest.plugin_label
    );
    match (&review.prior, &review.drift) {
        (None, _) => output.push_str("Status: NEEDS APPROVAL (first run)\n"),
        (Some(prior), None) => {
            output.push_str(&format!("Status: APPROVED at {}\n", prior.approved_at));
        }
        (Some(_), Some(drift)) if drift.blocks_admission => {
            output.push_str("Status: NEEDS APPROVAL (blocking permission expansion)\n");
        }
        (Some(_), Some(_)) => {
            output.push_str("Status: CHANGED (non-blocking; prior approval remains valid)\n");
        }
    }
    output.push_str("Grants:\n");
    if review.manifest.grants.is_empty() {
        output.push_str("  (none)\n");
    }
    for grant in &review.manifest.grants {
        output.push_str(&format!(
            "  - {}.{}\n    scopes: {}\n    optional: {}\n    reason: {}\n",
            grant.capability,
            grant.permission,
            render_scopes(&grant.scopes),
            if grant.optional { "yes" } else { "no" },
            grant.reason.as_deref().unwrap_or("(none)")
        ));
    }
    if let Some(drift) = &review.drift {
        output.push_str(if drift.blocks_admission {
            "Drift since approval (BLOCKING):\n"
        } else {
            "Drift since approval (non-blocking):\n"
        });
        for change in &drift.changes {
            output.push_str("  - ");
            output.push_str(&render_drift_change(change));
            output.push('\n');
        }
    }
    output
}

fn render_drift_change(change: &DriftChange) -> String {
    let permission = format!("{}.{}", change.capability, change.permission);
    match change.kind {
        DriftKind::NewGrant => format!(
            "now ALSO requests: {permission} → {} (NEW, BLOCKING)",
            render_optional_scopes(change.after.as_deref())
        ),
        DriftKind::ScopeWidened => {
            let before = change
                .before
                .as_deref()
                .unwrap_or_default()
                .iter()
                .collect::<BTreeSet<_>>();
            let added = change
                .after
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter(|scope| !before.contains(scope))
                .cloned()
                .collect::<Vec<_>>();
            format!(
                "now ALSO requests: {permission} → {} (WIDENED, BLOCKING)",
                render_scopes(&added)
            )
        }
        DriftKind::BecameRequired => {
            format!("now requires: {permission} (WAS OPTIONAL, BLOCKING)")
        }
        DriftKind::RemovedGrant => format!("no longer requests: {permission} (REMOVED)"),
        DriftKind::ScopeNarrowed => format!(
            "now requests fewer scopes: {permission} → {} (NARROWED)",
            render_optional_scopes(change.after.as_deref())
        ),
        DriftKind::BecameOptional => format!("now treats as optional: {permission} (OPTIONAL)"),
    }
}

fn render_optional_scopes(scopes: Option<&[String]>) -> String {
    scopes
        .map(render_scopes)
        .unwrap_or_else(|| "(none)".to_owned())
}

fn render_scopes(scopes: &[String]) -> String {
    if scopes.is_empty() {
        "(unscoped)".to_owned()
    } else {
        scopes.join(", ")
    }
}

fn plugin_list(builder: &AgentBuilder) -> Result<String, String> {
    let mut rows = Vec::new();
    for (id, component) in builder.plugins() {
        let roles = builder.plugin_roles(id)?;
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
}
