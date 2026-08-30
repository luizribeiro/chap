use crate::tool::ExecutionMode;
use serde::Deserialize;

#[cfg(test)]
use super::super::Config;

/// Settings for one plugin acting in the tools role.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolsSettings {
    /// How tools from this plugin may be scheduled.
    #[serde(default)]
    execution: ExecutionMode,
}

impl ToolsSettings {
    pub(crate) fn execution(&self) -> ExecutionMode {
        self.execution
    }
}

#[cfg(test)]
impl Config {
    pub(crate) fn plugin_tools_execution(&self, id: &str) -> ExecutionMode {
        self.plugin(id)
            .map(|plugin| plugin.role_settings())
            .map(|settings| settings.tools.execution())
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
