use crate::tool::ExecutionMode;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    plugins: BTreeMap<String, ConfiguredPlugin>,
    #[serde(default)]
    tools: ToolsConfig,
    #[serde(skip)]
    directory: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsConfig {
    #[serde(default)]
    execution: ExecutionMode,
    #[serde(default = "default_max_concurrency")]
    max_concurrency: NonZeroUsize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginToolsConfig {
    #[serde(default)]
    execution: ExecutionMode,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ContextChannel {
    #[default]
    Context,
    System,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginContextConfig {
    #[serde(default)]
    channel: ContextChannel,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredPlugin {
    component: PathBuf,
    #[serde(default)]
    tools: Option<PluginToolsConfig>,
    #[serde(default)]
    context: Option<PluginContextConfig>,
    #[serde(default)]
    settings: serde_json::Map<String, serde_json::Value>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let source = fs::read_to_string(path)
            .map_err(|error| format!("failed to read `{}`: {error}", path.display()))?;
        let mut config: Self = serde_json::from_str(&source)
            .map_err(|error| format!("failed to parse `{}`: {error}", path.display()))?;
        config.directory = path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        Ok(config)
    }

    pub fn plugins(&self) -> impl Iterator<Item = (&str, &ConfiguredPlugin)> {
        self.plugins
            .iter()
            .map(|(id, plugin)| (id.as_str(), plugin))
    }

    pub(crate) fn plugin(&self, id: &str) -> Option<&ConfiguredPlugin> {
        self.plugins.get(id)
    }

    pub(crate) fn execution_mode(&self, id: &str) -> ExecutionMode {
        self.plugin(id)
            .and_then(|plugin| plugin.tools.as_ref())
            .map(|tools| tools.execution)
            .unwrap_or_default()
    }

    pub(crate) fn tools(&self) -> &ToolsConfig {
        &self.tools
    }

    pub(crate) fn component_path(&self, plugin: &ConfiguredPlugin) -> PathBuf {
        self.directory.join(&plugin.component)
    }

    pub(crate) fn consent_path(&self) -> PathBuf {
        self.directory.join("consent.json")
    }
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            execution: ExecutionMode::default(),
            max_concurrency: default_max_concurrency(),
        }
    }
}

impl ToolsConfig {
    pub(crate) fn execution(&self) -> ExecutionMode {
        self.execution
    }

    pub(crate) fn max_concurrency(&self) -> NonZeroUsize {
        self.max_concurrency
    }
}

impl ConfiguredPlugin {
    pub fn component(&self) -> &Path {
        &self.component
    }

    pub(crate) fn settings(&self) -> Value {
        Value::Object(self.settings.clone())
    }

    pub(crate) fn context_channel(&self) -> ContextChannel {
        self.context
            .as_ref()
            .map(|context| context.channel)
            .unwrap_or_default()
    }

    pub(crate) fn has_section(&self, name: &str) -> bool {
        match name {
            "tools" => self.tools.is_some(),
            "context" => self.context.is_some(),
            _ => false,
        }
    }
}

fn default_max_concurrency() -> NonZeroUsize {
    NonZeroUsize::new(8).expect("default tool concurrency is nonzero")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugins() {
        let config: Config = serde_json::from_str(
            r#"{
                "plugins": {
                    "openai": {
                        "component": "./plugins/openai-compatible.wasm",
                        "settings": {
                            "base_url": "https://api.example.com/v1",
                            "model": "example-model"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let (id, plugin) = config.plugins().next().unwrap();
        assert_eq!(id, "openai");
        assert_eq!(
            plugin.component(),
            Path::new("./plugins/openai-compatible.wasm")
        );
        let settings = plugin.settings();
        assert_eq!(settings["base_url"], "https://api.example.com/v1");
        assert_eq!(settings["model"], "example-model");
    }

    #[test]
    fn defaults_tool_execution_when_the_section_is_absent() {
        let config: Config = serde_json::from_str("{}").unwrap();

        assert_eq!(config.tools.execution, ExecutionMode::Parallel);
        assert_eq!(config.tools.max_concurrency.get(), 8);
    }

    #[test]
    fn parses_sequential_tool_execution() {
        let config: Config = serde_json::from_str(
            r#"{
                "tools": {
                    "execution": "sequential"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(config.tools.execution, ExecutionMode::Sequential);
    }

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

        assert_eq!(config.execution_mode("kagi"), ExecutionMode::Sequential);
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

        assert_eq!(config.execution_mode("kagi"), ExecutionMode::Parallel);
    }

    #[test]
    fn parses_plugin_context_channels_and_defaults_to_context() {
        let config: Config = serde_json::from_str(
            r#"{
                "plugins": {
                    "default-context": {
                        "component": "context.wasm"
                    },
                    "operator-context": {
                        "component": "context.wasm",
                        "context": {
                            "channel": "system"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            config.plugin("default-context").unwrap().context_channel(),
            ContextChannel::Context
        );
        assert_eq!(
            config.plugin("operator-context").unwrap().context_channel(),
            ContextChannel::System
        );
    }

    #[test]
    fn rejects_unknown_plugin_context_settings() {
        for context in [
            r#"{ "channel": "assistant" }"#,
            r#"{ "channel": "context", "wrap": true }"#,
        ] {
            let source = format!(
                r#"{{
                    "plugins": {{
                        "example": {{
                            "component": "context.wasm",
                            "context": {context}
                        }}
                    }}
                }}"#
            );

            assert!(serde_json::from_str::<Config>(&source).is_err());
        }
    }

    #[test]
    fn rejects_top_level_plugin_execution() {
        let error = serde_json::from_str::<Config>(
            r#"{
                "plugins": {
                    "kagi": {
                        "component": "kagi.wasm",
                        "execution": "sequential"
                    }
                }
            }"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `execution`"));
    }

    #[test]
    fn rejects_zero_tool_concurrency() {
        let error = serde_json::from_str::<Config>(
            r#"{
                "tools": {
                    "max_concurrency": 0
                }
            }"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("nonzero usize"));
    }

    #[test]
    fn rejects_unknown_tool_settings() {
        let error = serde_json::from_str::<Config>(
            r#"{
                "tools": {
                    "concurrency": 8
                }
            }"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `concurrency`"));
    }

    #[test]
    fn passes_api_key_env_through_untouched() {
        let config: Config = serde_json::from_str(
            r#"{
                "plugins": {
                    "example": {
                        "component": "example.wasm",
                        "settings": {
                            "api_key_env": "EXAMPLE_API_KEY"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let settings = config.plugin("example").unwrap().settings();
        assert_eq!(settings["api_key_env"], "EXAMPLE_API_KEY");
        assert!(settings.get("api_key").is_none());
    }
}
