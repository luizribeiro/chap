use chap_plugin::tools::{ToolDefinition, ToolError, Tools};
use chap_plugin::vm::{ExecResult, Vm, Workspace, WorkspaceMount, WorkspaceSecret};
use chap_plugin::{MetadataSource, Needs, NoSettings, Plugin, ScopeRef, capabilities};
use serde::Deserialize;

const RUN: &str = "run";

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
        Ok(vec![run_definition(&workspace)])
    }

    async fn execute(&self, name: String, arguments: String) -> Result<String, ToolError> {
        if name != RUN {
            return Err(ToolError::InvalidInput(format!(
                "tool `{name}` is not provided by the workspace plugin"
            )));
        }
        let arguments = parse_arguments(&arguments)?;
        let (vm, _) = Vm::workspace().await.map_err(ToolError::from)?;
        let command_refs = arguments
            .command
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let output = vm
            .exec(&command_refs, None, None, None)
            .await
            .map_err(ToolError::from)?;
        Ok(format_output(output))
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

fn run_definition(workspace: &Workspace) -> ToolDefinition {
    let mounts = workspace
        .mounts
        .iter()
        .map(describe_mount)
        .collect::<Vec<_>>()
        .join(", ");
    let egress = workspace.egress.join(", ");
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
    ToolDefinition {
        name: RUN.to_owned(),
        description: format!(
            "Run a command in the agent's workspace VM using image {}. Guest mount points: [{mounts}]. Configured egress scopes: [{egress}]. The VM is reused across calls.{secrets}",
            workspace.image,
        ),
        parameters: r#"{
            "type":"object",
            "properties":{
                "command":{"type":"array","items":{"type":"string"},"description":"Command and arguments to run without a shell."}
            },
            "required":["command"],
            "additionalProperties":false
        }"#
        .to_owned(),
    }
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

fn format_output(output: ExecResult) -> String {
    let mut sections = vec![format!("Exit code: {}", output.exit_code)];
    if !output.stdout.is_empty() {
        sections.push(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    if !output.stderr.is_empty() {
        sections.push(format!(
            "stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if output.truncated {
        sections.push("[Command output was truncated.]".to_owned());
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
    command: Vec<String>,
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
        let description = run_definition(&workspace(false)).description;

        assert!(description.contains("docker.io/library/rust:1-alpine"));
        assert!(description.contains("/mnt/workspace (read-write)"));
        assert!(description.contains("0.0.0.0/0:443, 0.0.0.0/0:53"));
        assert!(description.contains("reused across calls"));
        assert!(
            run_definition(&workspace(true))
                .description
                .contains("/mnt/workspace (read-only)")
        );
    }

    #[test]
    fn describes_secret_names_and_hosts_only_when_configured() {
        let empty_description = run_definition(&workspace(false)).description;
        let mut configured = workspace(false);
        configured.secrets = vec![WorkspaceSecret {
            env: "GITHUB_TOKEN".into(),
            hosts: vec!["api.github.com".into(), "github.com".into()],
        }];

        let description = run_definition(&configured).description;

        assert!(description.contains("GITHUB_TOKEN (api.github.com, github.com)"));
        assert!(description.contains("placeholders"));
        assert!(!empty_description.contains("Secrets"));
    }

    #[test]
    fn parses_argv_only_run_arguments() {
        assert_eq!(
            parse_arguments(r#"{"command":["echo","hello"]}"#).unwrap(),
            RunArguments {
                command: vec!["echo".to_owned(), "hello".to_owned()],
            }
        );
        assert!(matches!(
            parse_arguments(r#"{"command":["pwd"],"shell":true}"#),
            Err(ToolError::InvalidInput(message)) if message.starts_with("invalid `run` arguments:")
        ));
    }

    #[test]
    fn publishes_the_run_tool_with_a_valid_parameter_schema() {
        let definition = run_definition(&workspace(false));

        assert_eq!(definition.name, RUN);
        let parameters: serde_json::Value = serde_json::from_str(&definition.parameters).unwrap();
        assert_eq!(parameters["type"], "object");
        assert_eq!(parameters["additionalProperties"], false);
        assert_eq!(parameters["required"], serde_json::json!(["command"]));
        assert_eq!(parameters["properties"]["command"]["type"], "array");
    }

    #[test]
    fn preserves_command_output_formatting() {
        assert_eq!(
            format_output(ExecResult {
                exit_code: 7,
                stdout: b"partial stdout".to_vec(),
                stderr: b"not found\n".to_vec(),
                truncated: true,
            }),
            "Exit code: 7\n\npartial stdout\n\nstderr:\nnot found\n\n\n[Command output was truncated.]"
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
