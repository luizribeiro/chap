//! Configuration is grouped by the qualifier that names each setting's scope:
//!
//! - Plugin settings live at `plugins.<id>.settings` and are read by the guest.
//! - Role settings live at `plugins.<id>.<role>` and are read by chap for one plugin role.
//! - Agent settings live at `agent` and are read by chap regardless of loaded plugins.

pub(crate) mod agent;
pub(crate) mod roles;

use agent::AgentSettings;
use lockgate::PluginId;
use roles::{ContextSettings, ToolsSettings};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LoadError {
    #[error("failed to read `{}`: {source}", path.display())]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse `{}`: {source}", path.display())]
    ParseConfig {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "failed to resolve config path `{}` as an absolute path: {source}",
        path.display()
    )]
    ResolveConfigPath {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "invalid instance name `{name}`: expected a non-empty value containing only A-Z, a-z, 0-9, '.', '_', or '-', other than '.' or '..'"
    )]
    InvalidName { name: String },
    #[error(
        "cannot locate CHAP state: neither XDG_STATE_HOME nor HOME is set to a non-empty value"
    )]
    StateDirectoryUnavailable,
    #[error("`agent.{capability}` is configured, but this build lacks {capability} support")]
    CapabilityUnsupported { capability: &'static str },
    #[error("failed to parse the `{section}` section: {source}")]
    InvalidAgentConfigSection {
        section: &'static str,
        source: serde_json::Error,
    },
}

/// The complete configuration loaded from `chap.json`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    name: Option<String>,
    #[serde(default)]
    plugins: BTreeMap<PluginId, ConfiguredPlugin>,
    #[serde(default)]
    agent: AgentSettings,
    #[serde(skip)]
    directory: PathBuf,
    #[serde(skip)]
    source_path: PathBuf,
}

/// One plugin's entry in `chap.json`: its component plus per-layer settings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfiguredPlugin {
    component: PathBuf,
    #[serde(default)]
    tools: Option<ToolsSettings>,
    #[serde(default)]
    context: Option<ContextSettings>,
    #[serde(default)]
    settings: serde_json::Map<String, serde_json::Value>,
}

impl Config {
    pub(crate) fn load(path: &Path) -> Result<Self, LoadError> {
        let source = fs::read_to_string(path).map_err(|source| LoadError::ReadConfig {
            path: path.to_path_buf(),
            source,
        })?;
        let mut config: Self =
            serde_json::from_str(&source).map_err(|source| LoadError::ParseConfig {
                path: path.to_path_buf(),
                source,
            })?;
        config.directory = path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        config.source_path = fs::canonicalize(path)
            .or_else(|_| std::path::absolute(path))
            .map_err(|source| LoadError::ResolveConfigPath {
                path: path.to_path_buf(),
                source,
            })?;
        config.validate_name()?;
        config.validate_agent_settings()?;
        Ok(config)
    }

    fn validate_name(&self) -> Result<(), LoadError> {
        let Some(name) = self.name() else {
            return Ok(());
        };
        if name.is_empty()
            || matches!(name, "." | "..")
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(LoadError::InvalidName {
                name: name.to_owned(),
            });
        }
        Ok(())
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn plugins(&self) -> impl Iterator<Item = (&PluginId, &ConfiguredPlugin)> {
        self.plugins.iter()
    }

    pub(crate) fn plugin(&self, plugin_id: &PluginId) -> Option<&ConfiguredPlugin> {
        self.plugins.get(plugin_id)
    }

    pub(crate) fn component_path(&self, plugin: &ConfiguredPlugin) -> PathBuf {
        self.directory.join(&plugin.component)
    }

    pub(crate) fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub(crate) fn consent_path(&self) -> Result<PathBuf, LoadError> {
        let xdg_state_home = env::var_os("XDG_STATE_HOME");
        let home = env::var_os("HOME");
        consent_path(
            &self.source_path,
            self.name(),
            xdg_state_home.as_deref(),
            home.as_deref(),
        )
    }
}

fn consent_path(
    config_path: &Path,
    name: Option<&str>,
    xdg_state_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<PathBuf, LoadError> {
    let state_root = if let Some(path) = xdg_state_home.filter(|path| !path.is_empty()) {
        PathBuf::from(path)
    } else if let Some(path) = home.filter(|path| !path.is_empty()) {
        PathBuf::from(path).join(".local/state")
    } else {
        return Err(LoadError::StateDirectoryUnavailable);
    };
    let instance_directory = match name {
        Some(name) => state_root.join("chap/named").join(name),
        None => {
            let digest = Sha256::digest(config_path.as_os_str().as_encoded_bytes());
            state_root.join("chap/by-path").join(format!("{digest:x}"))
        }
    };
    Ok(instance_directory.join("consent.json"))
}

impl ConfiguredPlugin {
    pub(crate) fn component(&self) -> &Path {
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
pub(crate) fn load_config(source: &str) -> Result<Config, LoadError> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chap.json");
    fs::write(&path, source).unwrap();
    Config::load(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_config_read_failures() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing.json");

        let error = Config::load(&path).unwrap_err();

        assert!(matches!(
            error,
            LoadError::ReadConfig {
                path: error_path,
                source,
            } if error_path == path && source.kind() == io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn instance_name_is_optional() {
        let config = load_config("{}").unwrap();

        assert_eq!(config.name(), None);
    }

    #[test]
    fn accepts_valid_instance_names() {
        for name in ["work", "Work_42", "team.alpha-beta"] {
            let config = load_config(&format!(r#"{{ "name": "{name}" }}"#)).unwrap();

            assert_eq!(config.name(), Some(name));
        }
    }

    #[test]
    fn named_instance_uses_xdg_state_home() {
        let path = consent_path(
            Path::new("/tmp/example/chap.json"),
            Some("work"),
            Some(OsStr::new("/state")),
            Some(OsStr::new("/home/example")),
        )
        .unwrap();

        assert_eq!(path, Path::new("/state/chap/named/work/consent.json"));
    }

    #[test]
    fn unnamed_instance_uses_full_config_path_digest() {
        let path = consent_path(
            Path::new("/tmp/example/chap.json"),
            None,
            Some(OsStr::new("/state")),
            None,
        )
        .unwrap();

        assert_eq!(
            path,
            Path::new(
                "/state/chap/by-path/0eeafb260cf31e5547071d36a94c1a43fda733c7767a64ad6d6387c81620fcac/consent.json"
            )
        );
    }

    #[test]
    fn empty_xdg_state_home_falls_back_to_home() {
        let path = consent_path(
            Path::new("/tmp/example/chap.json"),
            Some("work"),
            Some(OsStr::new("")),
            Some(OsStr::new("/home/example")),
        )
        .unwrap();

        assert_eq!(
            path,
            Path::new("/home/example/.local/state/chap/named/work/consent.json")
        );
    }

    #[test]
    fn state_location_requires_xdg_state_home_or_home() {
        for (xdg_state_home, home) in [
            (None, None),
            (Some(OsStr::new("")), None),
            (None, Some(OsStr::new(""))),
        ] {
            let error = consent_path(
                Path::new("/tmp/example/chap.json"),
                Some("work"),
                xdg_state_home,
                home,
            )
            .unwrap_err();

            assert!(matches!(error, LoadError::StateDirectoryUnavailable));
        }
    }

    #[test]
    fn load_stores_the_canonical_config_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chap.json");
        fs::write(&path, "{}").unwrap();

        let config = Config::load(&path).unwrap();

        assert_eq!(config.source_path(), fs::canonicalize(path).unwrap());
    }

    #[test]
    fn rejects_empty_instance_name() {
        let error = load_config(r#"{ "name": "" }"#).unwrap_err();

        assert!(matches!(
            error,
            LoadError::InvalidName { name } if name.is_empty()
        ));
    }

    #[test]
    fn rejects_instance_name_with_invalid_characters() {
        let error = load_config(r#"{ "name": "team/alpha" }"#).unwrap_err();

        assert!(matches!(
            error,
            LoadError::InvalidName { name } if name == "team/alpha"
        ));
    }

    #[test]
    fn rejects_non_ascii_instance_name() {
        let error = load_config(r#"{ "name": "café" }"#).unwrap_err();

        assert!(matches!(
            error,
            LoadError::InvalidName { name } if name == "café"
        ));
    }

    #[test]
    fn rejects_dot_instance_name() {
        let error = load_config(r#"{ "name": "." }"#).unwrap_err();

        assert!(matches!(
            error,
            LoadError::InvalidName { name } if name == "."
        ));
    }

    #[test]
    fn rejects_dot_dot_instance_name() {
        let error = load_config(r#"{ "name": ".." }"#).unwrap_err();

        assert!(matches!(
            error,
            LoadError::InvalidName { name } if name == ".."
        ));
    }

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
        assert_eq!(id, &PluginId::from("openai"));
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
        let error = load_config(
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

        let LoadError::ParseConfig { source, .. } = error else {
            panic!("expected config parse failure");
        };
        assert!(source.to_string().contains("unknown field `execution`"));
    }

    #[test]
    fn no_role_name_is_a_top_level_config_key() {
        for role in chap_wit::ROLES {
            let source = format!(r#"{{ "{}": {{}} }}"#, role.interface);
            let error = load_config(&source).unwrap_err();
            let LoadError::ParseConfig { source, .. } = error else {
                panic!("expected config parse failure");
            };
            assert!(
                source.to_string().contains("unknown field"),
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
            let plugin = config.plugin(&PluginId::from("example")).unwrap();

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

        let settings = config
            .plugin(&PluginId::from("example"))
            .unwrap()
            .settings();
        assert_eq!(settings["api_key_env"], "EXAMPLE_API_KEY");
        assert!(settings.get("api_key").is_none());
    }
}
