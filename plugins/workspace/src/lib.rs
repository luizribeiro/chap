use chap_plugin::tools::{ToolDefinition, ToolError, Tools};
use chap_plugin::vm::{ExecResult, Vm, Workspace, WorkspaceMount, WorkspaceSecret};
use chap_plugin::{MetadataSource, Needs, NoSettings, Plugin, ScopeRef, capabilities};
use serde::Deserialize;
use std::num::NonZeroU64;

const RUN: &str = "run";
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

struct WorkspacePlugin;

impl Plugin for WorkspacePlugin {
    const ID: &'static str = "workspace";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("Workspace");
    const DESCRIPTION: MetadataSource =
        MetadataSource::Explicit("Runs commands in the agent's workspace VM");
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs =
        Needs::required(&[capabilities::vm::MANAGE.need(&[ScopeRef::literal("workspace")])]);
    type Settings = NoSettings;

    fn new(_settings: Self::Settings) -> Self {
        Self
    }
}

impl Tools for WorkspacePlugin {
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        let (_, workspace) = futures::executor::block_on(Vm::workspace())
            .map_err(ToolError::from)
            .map_err(tool_error_message)?;
        Ok(vec![
            run_definition(&workspace).map_err(tool_error_message)?,
        ])
    }

    async fn execute(&self, name: String, arguments: String) -> Result<String, ToolError> {
        if name != RUN {
            return Err(ToolError::InvalidInput(format!(
                "tool `{name}` is not provided by the workspace plugin"
            )));
        }
        let arguments = parse_arguments(&arguments)?;
        let (vm, workspace) = Vm::workspace().await.map_err(ToolError::from)?;
        let cwd = working_directory(&workspace)?;
        let timeout_ms = command_timeout_ms(arguments.timeout_secs, workspace.exec_timeout_ms);
        let output = vm
            .exec(
                &["sh", "-c", &arguments.command],
                Some(cwd),
                None,
                Some(timeout_ms),
            )
            .await
            .map_err(ToolError::from)?;
        Ok(format_output(output, timeout_ms))
    }
}

fn tool_error_message(error: ToolError) -> String {
    match error {
        ToolError::InvalidInput(message)
        | ToolError::Denied(message)
        | ToolError::Failed(message)
        | ToolError::Fatal(message) => message,
    }
}

fn run_definition(workspace: &Workspace) -> Result<ToolDefinition, ToolError> {
    let cwd = working_directory(workspace)?;
    let mounts = workspace
        .mounts
        .iter()
        .map(describe_mount)
        .collect::<Vec<_>>()
        .join(", ");
    let egress = workspace.egress.join(", ");
    let default_timeout_ms = command_timeout_ms(None, workspace.exec_timeout_ms);
    let secrets = if workspace.secrets.is_empty() {
        String::new()
    } else {
        format!(
            " Secrets are available as environment variables whose placeholders the network gateway substitutes only for their allowed hosts: {}.",
            workspace
                .secrets
                .iter()
                .map(describe_secret)
                .collect::<Vec<_>>()
                .join("; ")
        )
    };
    Ok(ToolDefinition {
        name: RUN.to_owned(),
        description: format!(
            "Run a shell command with sh -c in the agent's workspace VM. The working directory is {cwd}. The image is {}. Guest mount points: [{mounts}]. Configured egress scopes: [{egress}]. The VM is reused across calls. Commands are killed when their timeout expires, and output produced up to that point is still returned. Output beyond the host's cap keeps the beginning and end with an omission marker between them; pipe through head, tail, or grep when you need a specific part. The default timeout is {} and the maximum is {}.{secrets}",
            workspace.image,
            describe_seconds(default_timeout_ms),
            describe_seconds(workspace.exec_timeout_ms),
        ),
        parameters: r#"{
            "type":"object",
            "properties":{
                "command":{"type":"string","description":"Shell command to run with sh -c."},
                "timeout_secs":{"type":"integer","minimum":1,"description":"Command timeout in seconds. Values above the reported maximum are clamped."}
            },
            "required":["command"],
            "additionalProperties":false
        }"#
        .to_owned(),
    })
}

fn working_directory(workspace: &Workspace) -> Result<&str, ToolError> {
    workspace
        .mounts
        .first()
        .map(|mount| mount.guest.as_str())
        .ok_or_else(|| ToolError::Failed("the workspace has no guest mount".to_owned()))
}

fn describe_seconds(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    let remainder = milliseconds % 1_000;
    if remainder == 0 {
        format!("{seconds} seconds")
    } else {
        format!("{seconds}.{remainder:03} seconds")
    }
}

fn command_timeout_ms(requested: Option<NonZeroU64>, maximum_ms: u64) -> u64 {
    requested
        .map_or(DEFAULT_TIMEOUT_MS, |seconds| {
            seconds.get().saturating_mul(1_000)
        })
        .min(maximum_ms)
}

fn describe_secret(secret: &WorkspaceSecret) -> String {
    format!("{} ({})", secret.env, secret.hosts.join(", "))
}

fn describe_mount(mount: &WorkspaceMount) -> String {
    let access = if mount.readonly {
        "read-only"
    } else {
        "read-write"
    };
    format!("{} ({access})", mount.guest)
}

fn format_output(output: ExecResult, timeout_ms: u64) -> String {
    let status = output.exit_code.map_or_else(
        || format!("Killed after {} (timeout)", describe_seconds(timeout_ms)),
        |exit_code| format!("Exit code: {exit_code}"),
    );
    let mut sections = vec![status];
    if !output.stdout.is_empty() {
        sections.push(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    if !output.stderr.is_empty() {
        sections.push(format!(
            "stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    sections.join("\n\n")
}

fn parse_arguments(arguments: &str) -> Result<RunArguments, ToolError> {
    serde_json::from_str(arguments)
        .map_err(|error| ToolError::InvalidInput(format!("invalid `run` arguments: {error}")))
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RunArguments {
    command: String,
    timeout_secs: Option<NonZeroU64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(readonly: bool) -> Workspace {
        Workspace {
            vm: "@workspace".into(),
            image: "docker.io/library/rust:1-alpine".into(),
            mounts: vec![WorkspaceMount {
                guest: "/mnt/workspace".into(),
                readonly,
            }],
            egress: vec!["0.0.0.0/0:443".into(), "0.0.0.0/0:53".into()],
            secrets: vec![],
            exec_timeout_ms: 30_000,
        }
    }

    #[test]
    fn publishes_a_closed_empty_settings_schema() {
        let schema = chap_plugin::__private::settings_schema::<WorkspacePlugin>();
        let schema: serde_json::Value = serde_json::from_str(&schema).unwrap();

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"], serde_json::json!({}));
        assert_eq!(schema["unevaluatedProperties"], false);
        assert!(serde_json::from_value::<NoSettings>(serde_json::json!({})).is_ok());
        assert!(
            serde_json::from_value::<NoSettings>(serde_json::json!({"image": "alpine"})).is_err()
        );
    }

    #[test]
    fn describes_the_workspace_configuration_and_reuse() {
        let description = run_definition(&workspace(false)).unwrap().description;

        assert!(description.contains("docker.io/library/rust:1-alpine"));
        assert!(description.contains("working directory is /mnt/workspace"));
        assert!(description.contains("/mnt/workspace (read-write)"));
        assert!(description.contains("0.0.0.0/0:443, 0.0.0.0/0:53"));
        assert!(description.contains("reused across calls"));
        assert!(description.contains("beginning and end with an omission marker"));
        assert!(description.contains("head, tail, or grep"));
        assert!(description.contains("default timeout is 30 seconds"));
        assert!(description.contains("maximum is 30 seconds"));
        assert!(
            run_definition(&workspace(true))
                .unwrap()
                .description
                .contains("/mnt/workspace (read-only)")
        );
    }

    #[test]
    fn describes_secret_names_and_hosts_only_when_configured() {
        let empty_description = run_definition(&workspace(false)).unwrap().description;
        let mut configured = workspace(false);
        configured.secrets = vec![WorkspaceSecret {
            env: "GITHUB_TOKEN".into(),
            hosts: vec!["api.github.com".into(), "github.com".into()],
        }];

        let description = run_definition(&configured).unwrap().description;

        assert!(description.contains("GITHUB_TOKEN (api.github.com, github.com)"));
        assert!(description.contains("placeholders"));
        assert!(!empty_description.contains("Secrets"));
    }

    #[test]
    fn parses_shell_command_run_arguments() {
        assert_eq!(
            parse_arguments(r#"{"command":"echo hello\npwd"}"#).unwrap(),
            RunArguments {
                command: "echo hello\npwd".to_owned(),
                timeout_secs: None,
            }
        );
        assert!(matches!(
            parse_arguments(r#"{"command":"pwd","shell":true}"#),
            Err(ToolError::InvalidInput(message)) if message.starts_with("invalid `run` arguments:")
        ));
        assert!(parse_arguments(r#"{"command":["pwd"]}"#).is_err());
        assert!(parse_arguments(r#"{"command":"pwd","timeout_secs":0}"#).is_err());
        assert!(parse_arguments(r#"{"command":"pwd","timeout_secs":1.5}"#).is_err());
    }

    #[test]
    fn publishes_the_run_tool_with_a_valid_parameter_schema() {
        let definition = run_definition(&workspace(false)).unwrap();

        assert_eq!(definition.name, RUN);
        let parameters: serde_json::Value = serde_json::from_str(&definition.parameters).unwrap();
        assert_eq!(parameters["type"], "object");
        assert_eq!(parameters["additionalProperties"], false);
        assert_eq!(parameters["required"], serde_json::json!(["command"]));
        assert_eq!(parameters["properties"]["command"]["type"], "string");
        assert_eq!(parameters["properties"]["timeout_secs"]["type"], "integer");
        assert_eq!(parameters["properties"]["timeout_secs"]["minimum"], 1);
    }

    #[test]
    fn defaults_to_120_seconds_when_the_reported_limit_is_higher() {
        assert_eq!(command_timeout_ms(None, 180_000), 120_000);
    }

    #[test]
    fn defaults_to_the_reported_limit_when_it_is_lower() {
        assert_eq!(command_timeout_ms(None, 30_000), 30_000);
    }

    #[test]
    fn uses_an_explicit_timeout_below_the_reported_limit() {
        assert_eq!(
            command_timeout_ms(Some(NonZeroU64::new(17).unwrap()), 30_000),
            17_000
        );
    }

    #[test]
    fn clamps_an_explicit_timeout_to_the_reported_limit() {
        assert_eq!(
            command_timeout_ms(Some(NonZeroU64::new(60).unwrap()), 30_000),
            30_000
        );
    }

    #[test]
    fn preserves_command_output_formatting() {
        assert_eq!(
            format_output(
                ExecResult {
                    exit_code: Some(7),
                    stdout: b"first stdout\n[... 42 bytes omitted ...]\nlast stdout".to_vec(),
                    stderr: b"not found\n".to_vec(),
                    truncated: true,
                },
                120_000
            ),
            "Exit code: 7\n\nfirst stdout\n[... 42 bytes omitted ...]\nlast stdout\n\nstderr:\nnot found\n"
        );
    }

    #[test]
    fn formats_timed_out_command_with_partial_output() {
        assert_eq!(
            format_output(
                ExecResult {
                    exit_code: None,
                    stdout: b"partial stdout".to_vec(),
                    stderr: b"still working\n".to_vec(),
                    truncated: false,
                },
                7_000
            ),
            "Killed after 7 seconds (timeout)\n\npartial stdout\n\nstderr:\nstill working\n"
        );
    }

    #[test]
    fn rejects_a_tool_name_the_plugin_does_not_provide() {
        let error = futures::executor::block_on(<WorkspacePlugin as Tools>::execute(
            &WorkspacePlugin,
            "shell".to_owned(),
            "{}".to_owned(),
        ))
        .unwrap_err();

        assert_eq!(
            error,
            ToolError::InvalidInput(
                "tool `shell` is not provided by the workspace plugin".to_owned()
            )
        );
    }
}

chap_plugin::plugin!(WorkspacePlugin: Tools, imports: Vm);
