use chap_plugin::exec::{Command, ExecResult};
use chap_plugin::tools::{ToolDefinition, ToolError, Tools};
use chap_plugin::{MetadataSource, Needs, Plugin, ScopeRef, capabilities};
use schemars::JsonSchema;
use serde::Deserialize;

const EXEC: &str = "exec";

struct Exec {
    settings: Settings,
}

impl Plugin for Exec {
    const ID: &'static str = "exec";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("Command runner");
    const DESCRIPTION: MetadataSource =
        MetadataSource::Explicit("Runs allowlisted commands in the project directory");
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs =
        Needs::required(&[capabilities::exec::RUN.need(&[ScopeRef::setting("/allowed_commands")])]);
    type Settings = Settings;

    fn new(settings: Self::Settings) -> Self {
        Self { settings }
    }
}

impl Tools for Exec {
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        let allowed_commands = self
            .settings
            .allowed_commands
            .iter()
            .map(|command| format!("`{command}`"))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(vec![ToolDefinition {
            name: EXEC.to_owned(),
            description: format!(
                "Run a command argv-style (without a shell) in the project root. Allowed command prefixes: {allowed_commands}."
            ),
            parameters: r#"{
                "type":"object",
                "properties":{
                    "program":{"type":"string","description":"Program name to run."},
                    "args":{"type":"array","items":{"type":"string"},"default":[],"description":"Arguments passed to the program as separate argv entries."},
                    "timeout_ms":{"type":"integer","minimum":0,"description":"Deadline in milliseconds for this command."}
                },
                "required":["program"],
                "additionalProperties":false
            }"#.to_owned(),
        }])
    }

    async fn execute(&self, name: String, arguments: String) -> Result<String, ToolError> {
        if name != EXEC {
            return Err(ToolError::InvalidInput(format!(
                "tool `{name}` is not provided by the exec plugin"
            )));
        }
        let arguments: ExecArguments = parse_arguments(&arguments)?;
        let timeout_ms = arguments.timeout_ms.or(self.settings.default_timeout_ms);
        chap_plugin::exec::run(
            Command {
                program: arguments.program,
                args: arguments.args,
            },
            timeout_ms,
        )
        .await
        .map(format_output)
        .map_err(Into::into)
    }
}

fn parse_arguments(arguments: &str) -> Result<ExecArguments, ToolError> {
    serde_json::from_str(arguments)
        .map_err(|error| ToolError::InvalidInput(format!("invalid `exec` arguments: {error}")))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Settings {
    /// Commands the plugin may run. Each entry is an argv prefix such as `git commit`.
    allowed_commands: Vec<String>,
    default_timeout_ms: Option<u64>,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct ExecArguments {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    timeout_ms: Option<u64>,
}

fn format_output(result: ExecResult) -> String {
    let mut sections = vec![format!("Exit code: {}", result.exit_code)];
    if !result.stdout.is_empty() {
        sections.push(result.stdout);
    }
    if !result.stderr.is_empty() {
        sections.push(format!("Stderr:\n{}", result.stderr));
    }
    if result.truncated {
        sections.push("[Command output was truncated.]".to_owned());
    }
    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(allowed_commands: &[&str]) -> Exec {
        <Exec as Plugin>::new(Settings {
            allowed_commands: allowed_commands
                .iter()
                .map(|command| (*command).to_owned())
                .collect(),
            default_timeout_ms: None,
        })
    }

    #[test]
    fn deserializes_required_and_optional_settings() {
        let settings: Settings = serde_json::from_str(
            r#"{"allowed_commands":["git status"],"default_timeout_ms":2500}"#,
        )
        .unwrap();

        assert_eq!(settings.allowed_commands, ["git status"]);
        assert_eq!(settings.default_timeout_ms, Some(2500));

        let settings: Settings = serde_json::from_str(r#"{"allowed_commands":["echo"]}"#).unwrap();
        assert_eq!(settings.allowed_commands, ["echo"]);
        assert_eq!(settings.default_timeout_ms, None);
    }

    #[test]
    fn rejects_missing_or_unknown_settings() {
        for json in ["{}", r#"{"allowed_commands":["echo"],"extra":true}"#] {
            assert!(serde_json::from_str::<Settings>(json).is_err());
        }
    }

    #[test]
    fn publishes_the_settings_object_schema() {
        let schema = chap_plugin::__private::settings_schema::<Exec>();
        let schema: serde_json::Value = serde_json::from_str(&schema).unwrap();

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["unevaluatedProperties"], false);
        assert_eq!(schema["required"], serde_json::json!(["allowed_commands"]));
        assert_eq!(
            schema["properties"]["allowed_commands"]["description"],
            "Commands the plugin may run. Each entry is an argv prefix such as `git commit`."
        );
        assert_eq!(
            schema["properties"]["default_timeout_ms"]["type"],
            serde_json::json!(["integer", "null"])
        );
    }

    #[test]
    fn parses_exec_arguments_and_defaults_args() {
        assert_eq!(
            parse_arguments(r#"{"program":"echo"}"#).unwrap(),
            ExecArguments {
                program: "echo".to_owned(),
                args: Vec::new(),
                timeout_ms: None,
            }
        );
        assert_eq!(
            parse_arguments(r#"{"program":"git","args":["status"],"timeout_ms":1000}"#).unwrap(),
            ExecArguments {
                program: "git".to_owned(),
                args: vec!["status".to_owned()],
                timeout_ms: Some(1000),
            }
        );
        assert!(matches!(
            parse_arguments(r#"{"program":"echo","shell":true}"#),
            Err(ToolError::InvalidInput(message))
                if message.starts_with("invalid `exec` arguments:")
        ));
    }

    #[test]
    fn formats_successful_and_nonzero_results() {
        assert_eq!(
            format_output(ExecResult {
                exit_code: 0,
                stdout: "hello\n".to_owned(),
                stderr: String::new(),
                truncated: false,
            }),
            "Exit code: 0\n\nhello\n"
        );
        assert_eq!(
            format_output(ExecResult {
                exit_code: 7,
                stdout: String::new(),
                stderr: "not found\n".to_owned(),
                truncated: false,
            }),
            "Exit code: 7\n\nStderr:\nnot found\n"
        );
    }

    #[test]
    fn reports_truncated_output() {
        assert_eq!(
            format_output(ExecResult {
                exit_code: 0,
                stdout: "partial".to_owned(),
                stderr: String::new(),
                truncated: true,
            }),
            "Exit code: 0\n\npartial\n\n[Command output was truncated.]"
        );
    }

    #[test]
    fn tool_description_lists_the_allowed_prefixes() {
        let definitions = <Exec as Tools>::definitions(&plugin(&["echo", "git status"])).unwrap();

        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].name, EXEC);
        assert!(definitions[0].description.contains("argv-style"));
        assert!(definitions[0].description.contains("project root"));
        assert!(definitions[0].description.contains("`echo`"));
        assert!(definitions[0].description.contains("`git status`"));
        serde_json::from_str::<serde_json::Value>(&definitions[0].parameters).unwrap();
    }
}

chap_plugin::plugin!(Exec: Tools, imports: Exec);
