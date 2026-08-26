use super::super::Config;
use crate::tool::ExecutionMode;
use serde::Deserialize;

/// Settings for one plugin acting in the tools role.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolsSettings {
    /// How tools from this plugin may be scheduled.
    #[serde(default)]
    execution: ExecutionMode,
}

impl Config {
    pub(crate) fn plugin_tools_execution(&self, id: &str) -> ExecutionMode {
        self.plugin(id)
            .and_then(|plugin| plugin.tools.as_ref())
            .map(|tools| tools.execution)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugin_tools_execution_override() {
        let config: Config = serde_json::from_str(
            r#"{
                "plugins": {
                    "kagi": {
                        "component": "kagi.wasm",
                        "tools": {
                            "execution": "sequential"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            config.plugin_tools_execution("kagi"),
            ExecutionMode::Sequential
        );
    }

    #[test]
    fn defaults_plugin_execution_when_the_tools_section_is_absent() {
        let config: Config = serde_json::from_str(
            r#"{
                "plugins": {
                    "kagi": {
                        "component": "kagi.wasm"
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            config.plugin_tools_execution("kagi"),
            ExecutionMode::Parallel
        );
    }
}
