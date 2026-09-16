use chap_plugin::capabilities::vm::Mount as MountScope;
use chap_plugin::tools::{ToolDefinition, ToolError, Tools};
use chap_plugin::vm::{ExecResult, Mount, Vm, VmConfig};
use chap_plugin::{MetadataSource, Needs, Plugin, ScopeRef, capabilities};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};

const RUN: &str = "run";
const WORKSPACE_MOUNT_POINT: &str = "/mnt/workspace";

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
        capabilities::vm::EGRESS.need(&[ScopeRef::setting("/allowed_egress")]),
        capabilities::vm::MANAGE.need(&[ScopeRef::literal("created-by-caller")]),
    ])
    .optional(&[capabilities::vm::MOUNT.need(&[
        ScopeRef::setting("/workspace"),
        ScopeRef::setting("/allowed_mounts"),
    ])]);
    type Settings = Settings;

    fn new(settings: Self::Settings) -> Self {
        Self { settings }
    }
}

impl Tools for Sandbox {
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        let guest_mounts = guest_mounts(&self.settings)
            .map(|(guest, mount)| describe_mount(&guest, mount))
            .collect::<Vec<_>>()
            .join(", ");
        let allowed_egress = self.settings.allowed_egress.join(", ");
        Ok(vec![ToolDefinition {
            name: RUN.to_owned(),
            description: format!(
                "Run a command in a reusable microVM with network access for configured egress scopes. Guest mount points: [{guest_mounts}]. Configured egress scopes: [{allowed_egress}]."
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
        }])
    }

    async fn execute(&self, name: String, arguments: String) -> Result<String, ToolError> {
        if name != RUN {
            return Err(ToolError::InvalidInput(format!(
                "tool `{name}` is not provided by the vm sandbox plugin"
            )));
        }
        let arguments = parse_arguments(&arguments)?;
        let config = vm_config(&self.settings);
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
        Ok(format_output(output))
    }
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

fn host_mount_point(host: &str) -> String {
    format!("/mnt/host{host}")
}

fn guest_mounts(settings: &Settings) -> impl Iterator<Item = (String, &MountScope)> {
    let workspace = settings
        .workspace
        .iter()
        .map(|mount| (WORKSPACE_MOUNT_POINT.to_owned(), mount));
    let host_paths = settings
        .allowed_mounts
        .iter()
        .map(|mount| (host_mount_point(mount.path()), mount));
    workspace.chain(host_paths)
}

fn describe_mount(guest: &str, mount: &MountScope) -> String {
    let access = if mount.readonly() {
        "read-only"
    } else {
        "read-write"
    };
    format!("{guest} ({access})")
}

fn vm_config(settings: &Settings) -> VmConfig {
    VmConfig {
        image: settings.image.clone(),
        mounts: guest_mounts(settings)
            .map(|(guest, mount)| Mount {
                host: mount.path().to_owned(),
                guest,
                readonly: mount.readonly(),
            })
            .collect(),
        egress: settings.allowed_egress.clone(),
        env: vec![],
    }
}

fn parse_arguments(arguments: &str) -> Result<RunArguments, ToolError> {
    serde_json::from_str(arguments)
        .map_err(|error| ToolError::InvalidInput(format!("invalid `run` arguments: {error}")))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Settings {
    /// OCI image reference used for the sandbox VM.
    #[serde(default = "default_image")]
    image: String,
    /// Host path mounted into the VM at `/mnt/workspace`: `ro:/path` mounts
    /// read-only and `rw:/path` mounts read-write.
    #[serde(default, deserialize_with = "deserialize_workspace")]
    #[schemars(with = "Option<String>")]
    workspace: Option<MountScope>,
    /// Further host paths mounted into the VM at `/mnt/host<path>`, with the
    /// same `ro:` and `rw:` prefixes.
    #[serde(default, deserialize_with = "deserialize_allowed_mounts")]
    #[schemars(with = "Vec<String>")]
    allowed_mounts: Vec<MountScope>,
    /// Network destinations the plugin may expose to a VM.
    allowed_egress: Vec<String>,
}

fn default_image() -> String {
    "docker.io/library/alpine:3.20".to_owned()
}

fn deserialize_workspace<'de, D>(deserializer: D) -> Result<Option<MountScope>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .as_deref()
        .map(parse_mount)
        .transpose()
}

fn deserialize_allowed_mounts<'de, D>(deserializer: D) -> Result<Vec<MountScope>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer)?
        .iter()
        .map(String::as_str)
        .map(parse_mount)
        .collect()
}

fn parse_mount<E: serde::de::Error>(mount: &str) -> Result<MountScope, E> {
    mount.parse::<MountScope>().map_err(|_| {
        E::custom(format!(
            "mount {mount:?} must be an absolute path prefixed with `ro:` or `rw:`"
        ))
    })
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RunArguments {
    command: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mounts(values: &[&str]) -> Vec<MountScope> {
        values.iter().map(|value| value.parse().unwrap()).collect()
    }

    fn settings(workspace: Option<&str>, allowed_mounts: &[&str]) -> Settings {
        Settings {
            image: default_image(),
            workspace: workspace.map(|value| value.parse().unwrap()),
            allowed_mounts: mounts(allowed_mounts),
            allowed_egress: vec![],
        }
    }

    fn plugin() -> Sandbox {
        <Sandbox as Plugin>::new(settings(Some("rw:/project"), &[]))
    }

    fn definition(plugin: &Sandbox) -> ToolDefinition {
        <Sandbox as Tools>::definitions(plugin).unwrap().remove(0)
    }

    #[test]
    fn publishes_the_settings_object_schema() {
        let schema = chap_plugin::__private::settings_schema::<Sandbox>();
        let schema: serde_json::Value = serde_json::from_str(&schema).unwrap();

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["unevaluatedProperties"], false);
        assert_eq!(schema["required"], serde_json::json!(["allowed_egress"]));
        assert_eq!(
            schema["properties"]["image"]["default"],
            "docker.io/library/alpine:3.20"
        );
        assert_eq!(
            schema["properties"]["workspace"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(schema["properties"]["allowed_mounts"]["type"], "array");
        assert_eq!(
            schema["properties"]["allowed_mounts"]["items"]["type"],
            "string"
        );
    }

    #[test]
    fn defaults_the_image_to_alpine() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "allowed_egress": [],
        }))
        .unwrap();

        assert_eq!(settings.image, "docker.io/library/alpine:3.20");
    }

    #[test]
    fn accepts_settings_without_any_mount() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "allowed_egress": [],
        }))
        .unwrap();

        assert_eq!(settings.workspace, None);
        assert!(settings.allowed_mounts.is_empty());
        assert!(vm_config(&settings).mounts.is_empty());
        let description = definition(&<Sandbox as Plugin>::new(settings)).description;
        assert!(
            description.contains("Guest mount points: []"),
            "{description}"
        );
    }

    #[test]
    fn rejects_invalid_mounts_when_settings_load() {
        for mount in [
            "project",
            "/project",
            "ro:",
            "rw:",
            "",
            "rw:/project/../secret",
        ] {
            for settings in [
                serde_json::json!({ "workspace": mount, "allowed_egress": [] }),
                serde_json::json!({ "allowed_mounts": [mount], "allowed_egress": [] }),
            ] {
                let message = serde_json::from_value::<Settings>(settings)
                    .unwrap_err()
                    .to_string();

                assert!(message.contains(&format!("{mount:?}")), "{message}");
                assert!(message.contains("absolute path"), "{message}");
            }
        }
    }

    #[test]
    fn parses_mount_scopes_when_settings_load() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "workspace": "rw:/project//",
            "allowed_mounts": ["ro:/srv/data", "rw:/var//log/"],
            "allowed_egress": [],
        }))
        .unwrap();

        assert_eq!(settings.workspace, Some("rw:/project".parse().unwrap()));
        assert_eq!(
            settings.allowed_mounts,
            mounts(&["ro:/srv/data", "rw:/var/log"])
        );
        assert!(settings.allowed_mounts[0].readonly());
        assert!(!settings.allowed_mounts[1].readonly());
    }

    #[test]
    fn configures_the_vm_with_allowed_egress() {
        let settings = Settings {
            allowed_egress: vec!["0.0.0.0/0:443".to_owned(), "10.0.0.0/8:80".to_owned()],
            ..settings(None, &[])
        };

        let config = vm_config(&settings);

        assert_eq!(config.egress, settings.allowed_egress);
    }

    #[test]
    fn mounts_the_workspace_before_the_other_host_paths() {
        let config = vm_config(&settings(Some("ro:/project"), &["rw:/var/log"]));

        assert_eq!(config.mounts.len(), 2);
        assert_eq!(config.mounts[0].host, "/project");
        assert_eq!(config.mounts[0].guest, "/mnt/workspace");
        assert!(config.mounts[0].readonly);
        assert_eq!(config.mounts[1].host, "/var/log");
        assert_eq!(config.mounts[1].guest, "/mnt/host/var/log");
        assert!(!config.mounts[1].readonly);
    }

    #[test]
    fn describes_each_mount_with_its_access_mode() {
        let plugin = <Sandbox as Plugin>::new(settings(Some("ro:/project"), &["rw:/var/log"]));

        let description = definition(&plugin).description;

        assert!(
            description.contains(
                "Guest mount points: [/mnt/workspace (read-only), /mnt/host/var/log (read-write)]"
            ),
            "{description}"
        );
    }

    #[test]
    fn keeps_host_paths_out_of_the_workspace_mount_point() {
        assert_eq!(host_mount_point("/workspace"), "/mnt/host/workspace");
        assert_eq!(host_mount_point("/var/log"), "/mnt/host/var/log");
    }

    #[test]
    fn parses_run_arguments() {
        assert_eq!(
            parse_arguments(r#"{"command":["echo","hello"]}"#).unwrap(),
            RunArguments {
                command: vec!["echo".to_owned(), "hello".to_owned()],
            }
        );
        assert!(matches!(
            parse_arguments(r#"{"command":["pwd"],"mounts":["/project"]}"#),
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
        assert!(parameters["properties"].get("mounts").is_none());
        assert!(definitions[0].description.contains("/mnt/workspace"));
    }

    #[test]
    fn formats_stdout_after_the_exit_code() {
        assert_eq!(
            format_output(ExecResult {
                exit_code: 0,
                stdout: b"hello\n".to_vec(),
                stderr: vec![],
                truncated: false,
            }),
            "Exit code: 0\n\nhello\n"
        );
    }

    #[test]
    fn includes_nonempty_stderr_after_stdout() {
        assert_eq!(
            format_output(ExecResult {
                exit_code: 7,
                stdout: b"partial stdout".to_vec(),
                stderr: b"not found\n".to_vec(),
                truncated: false,
            }),
            "Exit code: 7\n\npartial stdout\n\nstderr:\nnot found\n"
        );
    }

    #[test]
    fn reports_truncated_output_last() {
        assert_eq!(
            format_output(ExecResult {
                exit_code: 0,
                stdout: b"partial".to_vec(),
                stderr: vec![],
                truncated: true,
            }),
            "Exit code: 0\n\npartial\n\n[Command output was truncated.]"
        );
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
