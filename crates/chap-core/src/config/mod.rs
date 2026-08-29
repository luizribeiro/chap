//! Configuration is grouped by the qualifier that names each setting's scope:
//!
//! - Plugin settings live at `plugins.<id>.settings` and are read by the guest.
//! - Role settings live at `plugins.<id>.<role>` and are read by chap for one plugin role.
//! - Agent settings live at `agent` and are read by chap regardless of loaded plugins.

pub(crate) mod agent;
pub(crate) mod roles;

use agent::AgentSettings;
use roles::{ContextSettings, ToolsSettings};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// The complete settings loaded from `chap.json`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    plugins: BTreeMap<String, ConfiguredPlugin>,
    #[serde(default)]
    agent: AgentSettings,
    #[serde(skip)]
    directory: PathBuf,
}

/// Settings for one configured plugin and its roles.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredPlugin {
    component: PathBuf,
    #[serde(default)]
    tools: Option<ToolsSettings>,
    #[serde(default)]
    context: Option<ContextSettings>,
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
        config.validate_agent_settings()?;
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

    pub(crate) fn component_path(&self, plugin: &ConfiguredPlugin) -> PathBuf {
        self.directory.join(&plugin.component)
    }

    pub(crate) fn consent_path(&self) -> PathBuf {
        self.directory.join("consent.json")
    }
}

impl ConfiguredPlugin {
    pub fn component(&self) -> &Path {
        &self.component
    }

    pub(crate) fn settings(&self) -> Value {
        Value::Object(self.settings.clone())
    }

    pub(crate) fn has_section(&self, name: &str) -> bool {
        match name {
            "tools" => self.tools.is_some(),
            "context" => self.context.is_some(),
            _ => false,
        }
    }
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
    fn no_role_name_is_a_top_level_config_key() {
        for role in chap_wit::ROLES {
            let source = format!(r#"{{ "{}": {{}} }}"#, role.interface);
            let error = serde_json::from_str::<Config>(&source).unwrap_err();
            assert!(
                error.to_string().contains("unknown field"),
                "`{}` is both a role interface and a top-level chap.json key",
                role.interface,
            );
        }
    }

    #[test]
    fn every_parsed_role_section_is_wired_to_has_section() {
        for role in chap_wit::ROLES {
            let source = format!(
                r#"{{ "plugins": {{ "example": {{ "component": "x.wasm", "{}": {{}} }} }} }}"#,
                role.interface,
            );
            let Ok(config) = serde_json::from_str::<Config>(&source) else {
                continue;
            };
            let plugin = config.plugin("example").unwrap();

            assert!(
                plugin.has_section(role.interface),
                "role section `{}` parses but ConfiguredPlugin::has_section returns false",
                role.interface,
            );
        }
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
