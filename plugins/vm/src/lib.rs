use chap_plugin::tools::{ToolDefinition, ToolError, Tools};
use chap_plugin::vm::{Mount, Vm, VmConfig};
use chap_plugin::{MetadataSource, Needs, Plugin, ScopeRef, capabilities};
use schemars::JsonSchema;
use serde::Deserialize;

const RUN: &str = "run";

struct Sandbox {
    settings: Settings,
}

impl Plugin for Sandbox {
    const ID: &'static str = "sandbox";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("VM sandbox");
    const DESCRIPTION: MetadataSource =
        MetadataSource::Explicit("Runs commands in a reusable microVM sandbox");
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::required(&[
        capabilities::vm::CREATE.need(),
        capabilities::vm::MOUNT.need(&[ScopeRef::setting("/allowed_mounts")]),
        capabilities::vm::EGRESS.need(&[ScopeRef::setting("/allowed_egress")]),
        capabilities::vm::MANAGE.need(&[ScopeRef::literal("created-by-caller")]),
    ]);
    type Settings = Settings;

    fn new(settings: Self::Settings) -> Self {
        Self { settings }
    }
}

impl Tools for Sandbox {
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        let allowed_mounts = self.settings.allowed_mounts.join(", ");
        let allowed_egress = self.settings.allowed_egress.join(", ");
        Ok(vec![ToolDefinition {
            name: RUN.to_owned(),
            description: format!(
                "Run a command in a reusable microVM. Allowed host mounts: [{allowed_mounts}]. Configured egress scopes: [{allowed_egress}]."
            ),
            parameters: r#"{
                "type":"object",
                "properties":{
                    "command":{"type":"array","items":{"type":"string"},"description":"Command and arguments to run without a shell."},
                    "mounts":{"type":"array","items":{"type":"string"},"default":[],"description":"Host paths to mount read-only under /mnt."}
                },
                "required":["command"],
                "additionalProperties":false
            }"#
            .to_owned(),
        }])
    }

    async fn execute(&self, name: String, arguments: String) -> Result<String, ToolError> {
        if name != RUN {
            return Err(ToolError::InvalidInput(format!(
                "tool `{name}` is not provided by the vm sandbox plugin"
            )));
        }
        let arguments = parse_arguments(&arguments)?;
        let config = VmConfig {
            image: self.settings.image.clone(),
            mounts: arguments
                .mounts
                .iter()
                .map(|host| Mount {
                    host: host.clone(),
                    guest: format!("/mnt{host}"),
                    readonly: true,
                })
                .collect(),
            egress: vec![],
            env: vec![],
        };
        let vm = Vm::get_or_create("workspace", config)
            .await
            .map_err(ToolError::from)?;
        let command_refs = arguments
            .command
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let output = vm
            .exec(&command_refs, None, None, None)
            .await
            .map_err(ToolError::from)?;
        Ok(format!(
            "Exit code: {}\n\n{}",
            output.exit_code,
            String::from_utf8_lossy(&output.stdout)
        ))
    }
}

fn parse_arguments(arguments: &str) -> Result<RunArguments, ToolError> {
    serde_json::from_str(arguments)
        .map_err(|error| ToolError::InvalidInput(format!("invalid `run` arguments: {error}")))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Settings {
    /// OCI image reference used for the sandbox VM.
    image: String,
    /// Host paths the plugin may mount into a VM.
    allowed_mounts: Vec<String>,
    /// Network destinations the plugin may expose to a VM.
    allowed_egress: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RunArguments {
    command: Vec<String>,
    #[serde(default)]
    mounts: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin() -> Sandbox {
        <Sandbox as Plugin>::new(Settings {
            image: "ghcr.io/acme/alpine:latest".to_owned(),
            allowed_mounts: vec!["/project".to_owned()],
            allowed_egress: vec![],
        })
    }

    #[test]
    fn publishes_the_settings_object_schema() {
        let schema = chap_plugin::__private::settings_schema::<Sandbox>();
        let schema: serde_json::Value = serde_json::from_str(&schema).unwrap();

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["unevaluatedProperties"], false);
        assert_eq!(
            schema["required"],
            serde_json::json!(["image", "allowed_mounts", "allowed_egress"])
        );
    }

    #[test]
    fn parses_run_arguments_and_defaults_mounts() {
        assert_eq!(
            parse_arguments(r#"{"command":["echo","hello"]}"#).unwrap(),
            RunArguments {
                command: vec!["echo".to_owned(), "hello".to_owned()],
                mounts: vec![],
            }
        );
        assert_eq!(
            parse_arguments(r#"{"command":["pwd"],"mounts":["/project"]}"#).unwrap(),
            RunArguments {
                command: vec!["pwd".to_owned()],
                mounts: vec!["/project".to_owned()],
            }
        );
        assert!(matches!(
            parse_arguments(r#"{"command":["pwd"],"cwd":"/work"}"#),
            Err(ToolError::InvalidInput(message)) if message.starts_with("invalid `run` arguments:")
        ));
    }

    #[test]
    fn publishes_the_run_tool_with_a_valid_parameter_schema() {
        let definitions = <Sandbox as Tools>::definitions(&plugin()).unwrap();

        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].name, RUN);
        let parameters: serde_json::Value =
            serde_json::from_str(&definitions[0].parameters).unwrap();
        assert_eq!(parameters["type"], "object");
        assert_eq!(parameters["additionalProperties"], false);
        assert_eq!(parameters["required"], serde_json::json!(["command"]));
    }

    #[test]
    fn rejects_a_tool_name_the_plugin_does_not_provide() {
        let error = futures::executor::block_on(<Sandbox as Tools>::execute(
            &plugin(),
            "shell".to_owned(),
            "{}".to_owned(),
        ))
        .unwrap_err();

        assert_eq!(
            error,
            ToolError::InvalidInput(
                "tool `shell` is not provided by the vm sandbox plugin".to_owned()
            )
        );
    }
}

chap_plugin::plugin!(Sandbox: Tools, imports: Vm);
