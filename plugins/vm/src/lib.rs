use chap_plugin::capabilities::vm::Mount as MountScope;
use chap_plugin::tools::{ToolDefinition, ToolError, Tools};
use chap_plugin::vm::{ExecResult, Mount, Vm, VmConfig};
use chap_plugin::{MetadataSource, Needs, Plugin, ScopeRef, capabilities};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};

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
        let guest_mounts = self
            .settings
            .allowed_mounts
            .iter()
            .map(describe_mount)
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

fn guest_mount_point(host: &str) -> String {
    format!("/mnt{host}")
}

fn describe_mount(mount: &MountScope) -> String {
    let access = if mount.readonly() {
        "read-only"
    } else {
        "read-write"
    };
    format!("{} ({access})", guest_mount_point(mount.path()))
}

fn vm_config(settings: &Settings) -> VmConfig {
    VmConfig {
        image: settings.image.clone(),
        mounts: settings
            .allowed_mounts
            .iter()
            .map(|mount| Mount {
                host: mount.path().to_owned(),
                guest: guest_mount_point(mount.path()),
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
    /// Host paths mounted into the VM at `/mnt<path>`: `ro:/path` mounts
    /// read-only and `/path` mounts read-write.
    #[serde(deserialize_with = "deserialize_allowed_mounts")]
    #[schemars(with = "Vec<String>")]
    allowed_mounts: Vec<MountScope>,
    /// Network destinations the plugin may expose to a VM.
    allowed_egress: Vec<String>,
}

fn default_image() -> String {
    "docker.io/library/alpine:3.20".to_owned()
}

fn deserialize_allowed_mounts<'de, D>(deserializer: D) -> Result<Vec<MountScope>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer)?
        .iter()
        .map(|mount| {
            mount.parse::<MountScope>().map_err(|_| {
                serde::de::Error::custom(format!(
                    "allowed mount {mount:?} must be an absolute path, optionally prefixed with `ro:`"
                ))
            })
        })
        .collect()
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

    fn plugin() -> Sandbox {
        <Sandbox as Plugin>::new(Settings {
            image: default_image(),
            allowed_mounts: mounts(&["/project"]),
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
            serde_json::json!(["allowed_mounts", "allowed_egress"])
        );
        assert_eq!(
            schema["properties"]["image"]["default"],
            "docker.io/library/alpine:3.20"
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
            "allowed_mounts": [],
            "allowed_egress": [],
        }))
        .unwrap();

        assert_eq!(settings.image, "docker.io/library/alpine:3.20");
    }

    #[test]
    fn rejects_invalid_allowed_mounts_when_settings_load() {
        for mount in ["project", "ro:", "", "/project/../secret", "rw:/project"] {
            let error = serde_json::from_value::<Settings>(serde_json::json!({
                "allowed_mounts": [mount],
                "allowed_egress": [],
            }))
            .unwrap_err();
            let message = error.to_string();

            assert!(message.contains(&format!("{mount:?}")), "{message}");
            assert!(message.contains("absolute path"), "{message}");
        }
    }

    #[test]
    fn parses_mount_scopes_when_settings_load() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "allowed_mounts": ["ro:/project", "/var//log/"],
            "allowed_egress": [],
        }))
        .unwrap();

        assert_eq!(
            settings.allowed_mounts,
            mounts(&["ro:/project", "/var/log"])
        );
        assert!(settings.allowed_mounts[0].readonly());
        assert!(!settings.allowed_mounts[1].readonly());
    }

    #[test]
    fn configures_the_vm_with_allowed_egress() {
        let settings = Settings {
            image: default_image(),
            allowed_mounts: mounts(&[]),
            allowed_egress: vec!["0.0.0.0/0:443".to_owned(), "10.0.0.0/8:80".to_owned()],
        };

        let config = vm_config(&settings);

        assert_eq!(config.egress, settings.allowed_egress);
    }

    #[test]
    fn configures_the_vm_with_all_allowed_mounts() {
        let settings = Settings {
            image: default_image(),
            allowed_mounts: mounts(&["ro:/project", "/var/log"]),
            allowed_egress: vec![],
        };

        let config = vm_config(&settings);

        assert_eq!(config.mounts.len(), 2);
        assert_eq!(config.mounts[0].host, "/project");
        assert_eq!(config.mounts[0].guest, "/mnt/project");
        assert!(config.mounts[0].readonly);
        assert_eq!(config.mounts[1].host, "/var/log");
        assert_eq!(config.mounts[1].guest, "/mnt/var/log");
        assert!(!config.mounts[1].readonly);
    }

    #[test]
    fn describes_each_mount_with_its_access_mode() {
        let plugin = <Sandbox as Plugin>::new(Settings {
            image: default_image(),
            allowed_mounts: mounts(&["ro:/project", "/var/log"]),
            allowed_egress: vec![],
        });

        let definitions = <Sandbox as Tools>::definitions(&plugin).unwrap();

        assert!(
            definitions[0].description.contains(
                "Guest mount points: [/mnt/project (read-only), /mnt/var/log (read-write)]"
            ),
            "{}",
            definitions[0].description
        );
    }

    #[test]
    fn derives_guest_mount_points_from_absolute_host_paths() {
        assert_eq!(
            guest_mount_point("/Users/luiz/chap"),
            "/mnt/Users/luiz/chap"
        );
        assert_eq!(guest_mount_point("/var/log"), "/mnt/var/log");
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
        assert!(definitions[0].description.contains("/mnt/project"));
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
