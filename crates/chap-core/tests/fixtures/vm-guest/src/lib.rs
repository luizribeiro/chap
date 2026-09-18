use chap_plugin::capabilities::vm::Mount as MountScope;
use chap_plugin::tools::{ToolDefinition, ToolError, Tools};
use chap_plugin::vm::{ExecResult, Mount, Vm, VmConfig};
use chap_plugin::{MetadataSource, Needs, Plugin, ScopeRef, capabilities};
use schemars::JsonSchema;
use serde::Deserialize;

const RUN: &str = "run";
const IMAGE: &str = "docker.io/library/alpine:3.20";
const GUEST_MOUNT: &str = "/mnt/guest";

struct VmGuest {
    settings: Settings,
}

impl Plugin for VmGuest {
    const ID: &'static str = "vm-guest";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("VM guest fixture");
    const DESCRIPTION: MetadataSource =
        MetadataSource::Explicit("Exercises plugin-owned VM capability guards");
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

impl Tools for VmGuest {
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        Ok(vec![ToolDefinition {
            name: RUN.to_owned(),
            description: format!(
                "Run a command in a plugin-owned VM with a caller-selected host mount. Configured mount grants: [{}].",
                self.settings.allowed_mounts.join(", ")
            ),
            parameters: r#"{
                "type":"object",
                "properties":{
                    "mount":{"type":"string","description":"Host mount prefixed with ro: or rw:."},
                    "command":{"type":"array","items":{"type":"string"}}
                },
                "required":["mount","command"],
                "additionalProperties":false
            }"#
            .to_owned(),
        }])
    }

    async fn execute(&self, name: String, arguments: String) -> Result<String, ToolError> {
        if name != RUN {
            return Err(ToolError::InvalidInput(format!(
                "tool `{name}` is not provided by the vm guest fixture"
            )));
        }
        let arguments: RunArguments = serde_json::from_str(&arguments).map_err(|error| {
            ToolError::InvalidInput(format!("invalid `run` arguments: {error}"))
        })?;
        let mount = arguments.mount.parse::<MountScope>().map_err(|_| {
            ToolError::InvalidInput(format!(
                "mount {:?} must be an absolute path prefixed with `ro:` or `rw:`",
                arguments.mount
            ))
        })?;
        let vm = Vm::get_or_create(
            &format!("guest-{}", arguments.mount),
            VmConfig {
                image: IMAGE.to_owned(),
                mounts: vec![Mount {
                    host: mount.path().to_owned(),
                    guest: GUEST_MOUNT.to_owned(),
                    readonly: mount.readonly(),
                }],
                egress: self.settings.allowed_egress.clone(),
                env: vec![],
            },
        )
        .await
        .map_err(ToolError::from)?;
        let command = arguments
            .command
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        vm.exec(&command, None, None, None)
            .await
            .map(format_output)
            .map_err(ToolError::from)
    }
}

fn format_output(output: ExecResult) -> String {
    let status = output.exit_code.map_or_else(
        || "Timed out".to_owned(),
        |code| format!("Exit code: {code}"),
    );
    format!("{status}\n\n{}", String::from_utf8_lossy(&output.stdout))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Settings {
    allowed_mounts: Vec<String>,
    allowed_egress: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArguments {
    mount: String,
    command: Vec<String>,
}

chap_plugin::plugin!(VmGuest: Tools, imports: Vm);
