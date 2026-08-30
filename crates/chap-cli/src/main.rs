mod tui;

use chap_core::{
    AgentBuilder, ConsentError, DriftChange, DriftKind, ExportDrift, ExportDriftKind, LoadError,
    PluginConsentReview, PluginRefusal, PluginRefusalReason, SessionError, StartError,
};
use clap::{Args, Parser, Subcommand};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::ExitCode,
};
use unicode_width::UnicodeWidthStr;

const NO_PLUGINS_CONFIGURED: &str = "No plugins are configured.\n";

#[derive(Debug, Parser)]
#[command(version, about = "A plugin-powered coding agent")]
struct Cli {
    /// Configuration file to read.
    #[arg(long, env = "CHAP_CONFIG", default_value = "chap.json", global = true)]
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
    /// Approve the plugin's exact currently resolved permission manifest.
    Approve {
        /// Plugin instance to approve.
        instance_id: String,
    },
    /// Remove a plugin's stored approval.
    Deny {
        /// Plugin instance to deny.
        instance_id: String,
    },
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
    let builder = AgentBuilder::load(&cli.config).map_err(render_load_error)?;
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

fn render_load_error(error: LoadError) -> String {
    match error {
        LoadError::ExecUnsupported => {
            "agent.exec is configured, but this build lacks exec support; rebuild with the `exec` feature"
                .to_owned()
        }
        error => error.to_string(),
    }
}

fn render_start_error(error: StartError) -> String {
    match error {
        StartError::Refused(refusals) => render_plugin_refusals(&refusals),
        StartError::Internal(message) => message,
        error => error.to_string(),
    }
}

fn render_session_error(error: SessionError) -> String {
    match error {
        SessionError::Context(failures) => failures
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        error => error.to_string(),
    }
}

fn render_plugin_refusals(refusals: &[PluginRefusal]) -> String {
    refusals
        .iter()
        .map(render_plugin_refusal)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_plugin_refusal(refusal: &PluginRefusal) -> String {
    let id = &refusal.instance_id;
    match &refusal.reason {
        PluginRefusalReason::ApprovalRequired => format!(
            "plugin `{id}` from `{}` requires approval before admission{}",
            refusal.source_path.display(),
            approval_remedy(id)
        ),
        PluginRefusalReason::RenewedApprovalRequired { drift } => format!(
            "plugin `{id}` from `{}` {} and requires renewed approval before admission{}",
            refusal.source_path.display(),
            drift_description(drift),
            approval_remedy(id)
        ),
        PluginRefusalReason::UnsupportedRole {
            role,
            exported_interfaces,
        } => render_unsupported_role(id, &refusal.source_path, role, exported_interfaces),
        PluginRefusalReason::RoleConfigInvalid { role } => format!(
            "plugin `{id}` configures a `{role}` section, but its component does not export the {role} interface"
        ),
        PluginRefusalReason::ComponentLoad { source } => source.to_string(),
        _ => refusal.to_string(),
    }
}

fn approval_remedy(id: &str) -> String {
    format!("; run `chap grants review {id}` and then `chap grants approve {id}`")
}

fn drift_description(drift: &chap_core::DriftReport) -> &'static str {
    if drift
        .export_changes
        .iter()
        .any(|change| change.kind == ExportDriftKind::Gained)
    {
        "now exports interfaces it was not approved for"
    } else if drift.blocks_admission {
        "expanded its permission manifest"
    } else {
        "reported a changed permission manifest"
    }
}

fn render_consent_error(error: ConsentError) -> String {
    match error {
        ConsentError::StateLocation(source) | ConsentError::HostConfiguration(source) => {
            render_load_error(source)
        }
        ConsentError::UnsupportedRole {
            plugin,
            path,
            role,
            exported_interfaces,
        } => render_unsupported_role(&plugin, &path, &role, &exported_interfaces),
        error => error.to_string(),
    }
}

fn render_unsupported_role(
    plugin: &str,
    path: &Path,
    role: &str,
    exported_interfaces: &[String],
) -> String {
    format!(
        "plugin `{plugin}` from `{}` does not implement a supported role; expected an export from the `{role}` package, but the component exports {}",
        path.display(),
        render_exported_interfaces(exported_interfaces)
    )
}

fn render_exported_interfaces(interfaces: &[String]) -> String {
    if interfaces.is_empty() {
        return "no interfaces".to_owned();
    }
    interfaces
        .iter()
        .map(|interface| format!("`{interface}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn grants_review(
    builder: &AgentBuilder,
    instance_id: Option<&str>,
) -> Result<String, String> {
    let consent_path = builder.consent_path().map_err(render_load_error)?;
    let mut output = format!("Consent store: {}\n", consent_path.display());
    let ids = match instance_id {
        Some(id) => vec![id.to_owned()],
        None => builder
            .plugins()
            .map(|(id, _)| id.to_owned())
            .collect::<Vec<_>>(),
    };
    if ids.is_empty() {
        output.push_str(NO_PLUGINS_CONFIGURED);
        return Ok(output);
    }

    output.push_str(
        "Concrete scopes come from chap.json; approval grants this exact resolved manifest.\n",
    );
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        output.push_str(&render_grant_review(
            &builder
                .review_plugin(id)
                .await
                .map_err(render_consent_error)?,
        ));
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
    output.push_str("Exports:\n");
    if review.manifest.exported_interfaces.is_empty() {
        output.push_str("  (none)\n");
    }
    for exported_interface in &review.manifest.exported_interfaces {
        output.push_str(&format!("  - {exported_interface}\n"));
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
        for change in &drift.export_changes {
            output.push_str("  - ");
            output.push_str(&render_export_drift(change));
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

fn render_export_drift(change: &ExportDrift) -> String {
    match change.kind {
        ExportDriftKind::Gained => format!(
            "now ALSO exports: {} (NEW ROLE, BLOCKING)",
            change.after.join(", ")
        ),
        ExportDriftKind::Lost => {
            format!("no longer exports: {} (REMOVED)", change.before.join(", "))
        }
        ExportDriftKind::VersionChanged => format!(
            "exports {} at {} (was {})",
            change.name,
            change.after.join(", "),
            change.before.join(", ")
        ),
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
    use chap_core::ContextFailure;
    use chap_core::{
        ConsentManifest, ConsentRecord, DriftReport, GrantReview, PluginConsentReview,
    };
    use clap::CommandFactory;
    use std::{ffi::OsStr, io};

    fn consent_record(instance_id: &str, digest_byte: char) -> ConsentRecord {
        serde_json::from_value(serde_json::json!({
            "instance_id": instance_id,
            "fingerprint": format!("sha256:{}", digest_byte.to_string().repeat(64)),
            "grants": [{
                "capability": "net",
                "permission": "egress",
                "scopes": ["https://api.example.com"],
                "optional": false,
                "reason": "Call the configured API"
            }],
            "approved_at": "2026-08-19T14:30:00Z"
        }))
        .unwrap()
    }

    fn manifest(scopes: &[&str], digest_byte: char) -> ConsentManifest {
        let fingerprint = consent_record("example", digest_byte).request_digest;
        ConsentManifest {
            instance_id: "example".to_owned(),
            plugin_label: "Example provider".to_owned(),
            request_digest: fingerprint,
            component_digest: format!("sha256:{}", digest_byte.to_string().repeat(64)),
            exported_interfaces: vec!["chap:agent/provider@0.2.0".to_owned()],
            grants: vec![GrantReview {
                capability: "net".to_owned(),
                permission: "egress".to_owned(),
                scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
                optional: false,
                reason: Some("Call the configured API".to_owned()),
            }],
        }
    }

    fn refusal(id: &str, component: &str, reason: PluginRefusalReason) -> PluginRefusal {
        PluginRefusal {
            instance_id: id.to_owned(),
            source_path: PathBuf::from(component),
            reason,
        }
    }

    #[test]
    fn starts_the_tui_when_no_subcommand_is_given() {
        let cli = Cli::try_parse_from(["chap"]).unwrap();

        assert!(cli.command.is_none());
    }

    #[test]
    fn config_flag_has_an_environment_default_before_the_file_default() {
        let command = Cli::command();
        let argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == "config")
            .unwrap();

        assert_eq!(argument.get_env(), Some(OsStr::new("CHAP_CONFIG")));
        assert_eq!(argument.get_default_values(), [OsStr::new("chap.json")]);

        let cli = Cli::try_parse_from(["chap", "--config", "explicit.json"]).unwrap();
        assert_eq!(cli.config, PathBuf::from("explicit.json"));
    }

    #[test]
    fn renders_config_load_failure_lines() {
        assert_eq!(
            render_load_error(LoadError::ReadConfig {
                path: PathBuf::from("missing.json"),
                source: io::Error::new(io::ErrorKind::PermissionDenied, "permission denied"),
            }),
            "failed to read `missing.json`: permission denied"
        );
        assert_eq!(
            render_load_error(LoadError::ParseConfig {
                path: PathBuf::from("broken.json"),
                source: serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
            }),
            "failed to parse `broken.json`: EOF while parsing an object at line 1 column 1"
        );
        assert_eq!(
            render_load_error(LoadError::ResolveConfigPath {
                path: PathBuf::from("relative/chap.json"),
                source: io::Error::new(io::ErrorKind::NotFound, "current directory missing"),
            }),
            "failed to resolve config path `relative/chap.json` as an absolute path: current directory missing"
        );
        assert_eq!(
            render_load_error(LoadError::InvalidName {
                name: "team/alpha".to_owned(),
            }),
            "invalid instance name `team/alpha`: expected a non-empty value containing only A-Z, a-z, 0-9, '.', '_', or '-', other than '.' or '..'"
        );
        assert_eq!(
            render_load_error(LoadError::StateDirectoryUnavailable),
            "cannot locate CHAP state: neither XDG_STATE_HOME nor HOME is set to a non-empty value"
        );
        assert_eq!(
            render_load_error(LoadError::InvalidExecConfig {
                source: serde_json::from_str::<u64>(r#""soon""#).unwrap_err(),
            }),
            "failed to parse the `agent.exec` config section: invalid type: string \"soon\", expected u64 at line 1 column 6"
        );
    }

    #[test]
    fn renders_the_exec_rebuild_hint() {
        assert_eq!(
            render_load_error(LoadError::ExecUnsupported),
            "agent.exec is configured, but this build lacks exec support; rebuild with the `exec` feature"
        );
    }

    #[test]
    fn renders_context_failures_one_per_line() {
        let error = SessionError::Context(vec![
            ContextFailure {
                plugin: "alpha".to_owned(),
                error: "unavailable".to_owned(),
            },
            ContextFailure {
                plugin: "bravo".to_owned(),
                error: "timed out after 10s".to_owned(),
            },
        ]);

        assert_eq!(
            render_session_error(error),
            "context plugin `alpha` failed: unavailable\ncontext plugin `bravo` failed: timed out after 10s"
        );
        assert_eq!(
            render_session_error(SessionError::ProviderNotConfigured {
                provider: "missing".to_owned(),
            }),
            "provider plugin `missing` is not configured"
        );
    }

    #[test]
    fn still_parses_plugin_commands() {
        let cli = Cli::try_parse_from(["chap", "plugins", "list"]).unwrap();

        assert!(matches!(
            cli.command,
            Some(Command::Plugins(Plugins {
                command: PluginsCommand::List
            }))
        ));
    }

    #[test]
    fn parses_grants_review_approve_and_deny_commands() {
        let review = Cli::try_parse_from(["chap", "grants", "review"]).unwrap();
        let approve = Cli::try_parse_from(["chap", "grants", "approve", "openai"]).unwrap();
        let deny = Cli::try_parse_from(["chap", "grants", "deny", "openai"]).unwrap();

        assert!(matches!(
            review.command,
            Some(Command::Grants(Grants {
                command: GrantsCommand::Review { instance_id: None }
            }))
        ));
        assert!(matches!(
            approve.command,
            Some(Command::Grants(Grants {
                command: GrantsCommand::Approve { instance_id }
            })) if instance_id == "openai"
        ));
        assert!(matches!(
            deny.command,
            Some(Command::Grants(Grants {
                command: GrantsCommand::Deny { instance_id }
            })) if instance_id == "openai"
        ));
    }

    #[test]
    fn renders_admission_remedies_one_per_line() {
        let refusals = [
            refusal(
                "alpha",
                "plugins/alpha.wasm",
                PluginRefusalReason::ApprovalRequired,
            ),
            refusal(
                "bravo",
                "plugins/bravo.wasm",
                PluginRefusalReason::RenewedApprovalRequired {
                    drift: DriftReport {
                        changes: Vec::new(),
                        export_changes: Vec::new(),
                        blocks_admission: true,
                    },
                },
            ),
        ];

        assert_eq!(
            render_start_error(StartError::Refused(refusals.into())),
            "plugin `alpha` from `plugins/alpha.wasm` requires approval before admission; run `chap grants review alpha` and then `chap grants approve alpha`\n\
             plugin `bravo` from `plugins/bravo.wasm` expanded its permission manifest and requires renewed approval before admission; run `chap grants review bravo` and then `chap grants approve bravo`"
        );
        assert_eq!(
            render_start_error(StartError::Internal("host setup failed".to_owned())),
            "host setup failed"
        );
    }

    #[test]
    fn renders_gained_exports_as_renewed_approval() {
        let refusal = refusal(
            "example",
            "plugins/example.wasm",
            PluginRefusalReason::RenewedApprovalRequired {
                drift: DriftReport {
                    changes: Vec::new(),
                    export_changes: vec![ExportDrift {
                        name: "chap:agent/tools".to_owned(),
                        kind: ExportDriftKind::Gained,
                        before: Vec::new(),
                        after: vec!["chap:agent/tools@0.3.0".to_owned()],
                    }],
                    blocks_admission: true,
                },
            },
        );

        assert_eq!(
            render_plugin_refusal(&refusal),
            "plugin `example` from `plugins/example.wasm` now exports interfaces it was not approved for and requires renewed approval before admission; run `chap grants review example` and then `chap grants approve example`"
        );
    }

    #[test]
    fn renders_nonblocking_drift_as_renewed_approval() {
        let refusal = refusal(
            "example",
            "plugins/example.wasm",
            PluginRefusalReason::RenewedApprovalRequired {
                drift: DriftReport {
                    changes: Vec::new(),
                    export_changes: Vec::new(),
                    blocks_admission: false,
                },
            },
        );

        assert_eq!(
            render_plugin_refusal(&refusal),
            "plugin `example` from `plugins/example.wasm` reported a changed permission manifest and requires renewed approval before admission; run `chap grants review example` and then `chap grants approve example`"
        );
    }

    #[test]
    fn renders_structured_unsupported_role_details() {
        let refusal = refusal(
            "example",
            "plugins/example.wasm",
            PluginRefusalReason::UnsupportedRole {
                role: "chap:agent@0.3.0".to_owned(),
                exported_interfaces: vec![
                    "lockgate:config/schema".to_owned(),
                    "example:plugin/unsupported@1.0.0".to_owned(),
                ],
            },
        );

        assert_eq!(
            render_plugin_refusal(&refusal),
            "plugin `example` from `plugins/example.wasm` does not implement a supported role; expected an export from the `chap:agent@0.3.0` package, but the component exports `lockgate:config/schema`, `example:plugin/unsupported@1.0.0`"
        );
    }

    #[test]
    fn renders_structured_role_config_and_component_failures() {
        let role = refusal(
            "example",
            "plugins/example.wasm",
            PluginRefusalReason::RoleConfigInvalid {
                role: "tools".to_owned(),
            },
        );
        let load = refusal(
            "missing",
            "plugins/missing.wasm",
            PluginRefusalReason::ComponentLoad {
                source: Box::new(ConsentError::ReadPlugin {
                    plugin: "missing".to_owned(),
                    path: PathBuf::from("plugins/missing.wasm"),
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, "component missing"),
                }),
            },
        );

        assert_eq!(
            render_plugin_refusal(&role),
            "plugin `example` configures a `tools` section, but its component does not export the tools interface"
        );
        assert_eq!(
            render_plugin_refusal(&load),
            "failed to read plugin `missing` from `plugins/missing.wasm`: component missing"
        );
    }

    #[tokio::test]
    async fn renders_load_plugin_component_failures() {
        let directory = tempfile::tempdir().unwrap();
        let component = directory.path().join("broken.wasm");
        std::fs::write(&component, []).unwrap();
        let config_path = directory.path().join("chap.json");
        std::fs::write(
            &config_path,
            r#"{
                "plugins": {
                    "broken": {
                        "component": "broken.wasm"
                    }
                }
            }"#,
        )
        .unwrap();
        let builder = AgentBuilder::load(&config_path)
            .unwrap()
            .state_dir(directory.path());
        let error = builder.review_plugin("broken").await.unwrap_err();

        assert!(matches!(
            &error,
            ConsentError::LoadPlugin { plugin, path, .. }
                if plugin == "broken" && path == &component
        ));
        let refusal = PluginRefusal {
            instance_id: "broken".to_owned(),
            source_path: component.clone(),
            reason: PluginRefusalReason::ComponentLoad {
                source: Box::new(error),
            },
        };

        assert_eq!(
            render_plugin_refusal(&refusal),
            format!(
                "failed to load plugin `broken` from `{}`: input is not a valid WebAssembly component: unexpected end-of-file (at offset 0x0)",
                component.display()
            )
        );
    }

    #[test]
    fn renders_consent_errors_with_export_details() {
        let error = ConsentError::UnsupportedRole {
            plugin: "example".to_owned(),
            path: PathBuf::from("plugins/example.wasm"),
            role: "chap:agent@0.3.0".to_owned(),
            exported_interfaces: Vec::new(),
        };

        assert_eq!(
            render_consent_error(error),
            "plugin `example` from `plugins/example.wasm` does not implement a supported role; expected an export from the `chap:agent@0.3.0` package, but the component exports no interfaces"
        );
    }

    #[tokio::test]
    async fn grants_review_starts_with_the_consent_store_path() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("chap.json");
        std::fs::write(&config_path, "{}").unwrap();
        let state_dir = directory.path().join("state");
        let builder = AgentBuilder::load(&config_path)
            .unwrap()
            .state_dir(&state_dir);

        let output = grants_review(&builder, None).await.unwrap();

        assert!(
            output.starts_with(&format!(
                "Consent store: {}\n",
                state_dir.join("consent.json").display()
            )),
            "{output}"
        );
    }

    #[test]
    fn renders_every_manifest_field_for_review() {
        let output = render_grant_review(&PluginConsentReview {
            manifest: manifest(&["https://api.example.com"], '1'),
            prior: None,
            drift: None,
        });

        assert!(output.contains("Instance: example"), "{output}");
        assert!(output.contains("Plugin: Example provider"), "{output}");
        assert!(output.contains("NEEDS APPROVAL (first run)"), "{output}");
        assert!(
            output.contains("Exports:\n  - chap:agent/provider@0.2.0\nGrants:"),
            "{output}"
        );
        assert!(output.contains("net.egress"), "{output}");
        assert!(
            output.contains("scopes: https://api.example.com"),
            "{output}"
        );
        assert!(output.contains("optional: no"), "{output}");
        assert!(
            output.contains("reason: Call the configured API"),
            "{output}"
        );
    }

    #[test]
    fn renders_an_empty_exports_section() {
        let mut manifest = manifest(&["https://api.example.com"], '1');
        manifest.exported_interfaces.clear();

        let output = render_grant_review(&PluginConsentReview {
            manifest,
            prior: None,
            drift: None,
        });

        assert!(output.contains("Exports:\n  (none)\nGrants:"), "{output}");
    }

    #[test]
    fn renders_blocking_scope_expansion_as_a_drift_delta() {
        let prior = consent_record("example", '1');
        let output = render_grant_review(&PluginConsentReview {
            manifest: manifest(
                &["https://api.example.com", "https://evil.example.com"],
                '2',
            ),
            prior: Some(prior),
            drift: Some(DriftReport {
                changes: vec![DriftChange {
                    capability: "net".to_owned(),
                    permission: "egress".to_owned(),
                    kind: DriftKind::ScopeWidened,
                    before: Some(vec!["https://api.example.com".to_owned()]),
                    after: Some(vec![
                        "https://api.example.com".to_owned(),
                        "https://evil.example.com".to_owned(),
                    ]),
                }],
                export_changes: Vec::new(),
                blocks_admission: true,
            }),
        });

        assert!(output.contains("blocking permission expansion"), "{output}");
        assert!(
            output.contains("Drift since approval (BLOCKING)"),
            "{output}"
        );
        assert!(
            output.contains(
                "now ALSO requests: net.egress → https://evil.example.com (WIDENED, BLOCKING)"
            ),
            "{output}"
        );
    }

    #[test]
    fn renders_all_export_drift_deltas() {
        let output = render_grant_review(&PluginConsentReview {
            manifest: manifest(&["https://api.example.com"], '2'),
            prior: Some(consent_record("example", '1')),
            drift: Some(DriftReport {
                changes: Vec::new(),
                export_changes: vec![
                    ExportDrift {
                        name: "chap:agent/tools".to_owned(),
                        kind: ExportDriftKind::Gained,
                        before: Vec::new(),
                        after: vec!["chap:agent/tools@0.2.0".to_owned()],
                    },
                    ExportDrift {
                        name: "chap:agent/provider".to_owned(),
                        kind: ExportDriftKind::Lost,
                        before: vec!["chap:agent/provider@0.1.0".to_owned()],
                        after: Vec::new(),
                    },
                    ExportDrift {
                        name: "chap:agent/context".to_owned(),
                        kind: ExportDriftKind::VersionChanged,
                        before: vec!["chap:agent/context@0.1.0".to_owned()],
                        after: vec![
                            "chap:agent/context@0.2.0".to_owned(),
                            "chap:agent/context@0.3.0".to_owned(),
                        ],
                    },
                ],
                blocks_admission: true,
            }),
        });

        assert!(
            output.contains("now ALSO exports: chap:agent/tools@0.2.0 (NEW ROLE, BLOCKING)"),
            "{output}"
        );
        assert!(
            output.contains("no longer exports: chap:agent/provider@0.1.0 (REMOVED)"),
            "{output}"
        );
        assert!(
            output.contains(
                "exports chap:agent/context at chap:agent/context@0.2.0, chap:agent/context@0.3.0 (was chap:agent/context@0.1.0)"
            ),
            "{output}"
        );
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
