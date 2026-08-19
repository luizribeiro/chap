use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    plugins: BTreeMap<String, Plugin>,
    #[serde(skip)]
    directory: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plugin {
    component: PathBuf,
    #[serde(default)]
    settings: toml::Table,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let source = fs::read_to_string(path)
            .map_err(|error| format!("failed to read `{}`: {error}", path.display()))?;
        let mut config: Self = toml::from_str(&source)
            .map_err(|error| format!("failed to parse `{}`: {error}", path.display()))?;
        config.directory = path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        Ok(config)
    }

    pub fn plugins(&self) -> impl Iterator<Item = (&str, &Plugin)> {
        self.plugins
            .iter()
            .map(|(id, plugin)| (id.as_str(), plugin))
    }

    pub(crate) fn plugin(&self, id: &str) -> Option<&Plugin> {
        self.plugins.get(id)
    }

    pub(crate) fn component_path(&self, plugin: &Plugin) -> PathBuf {
        self.directory.join(&plugin.component)
    }
}

impl Plugin {
    pub fn component(&self) -> &Path {
        &self.component
    }

    pub(crate) fn settings(&self, id: &str) -> Result<Value, String> {
        let mut settings = self.settings.clone();
        let api_key_env = settings.remove("api-key-env");
        if !settings.contains_key("api-key")
            && let Some(variable) = api_key_env
        {
            let variable = variable
                .as_str()
                .ok_or_else(|| format!("plugin `{id}` setting `api-key-env` must be a string"))?;
            let api_key = env::var(variable).map_err(|error| {
                format!(
                    "failed to read API key for plugin `{id}` from environment variable `{variable}`: {error}"
                )
            })?;
            settings.insert("api-key".to_owned(), toml::Value::String(api_key));
        }
        serde_json::to_value(settings)
            .map_err(|error| format!("failed to encode settings for plugin `{id}`: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugins() {
        let config: Config = toml::from_str(
            r#"
[plugins.openai]
component = "./plugins/openai-compatible.wasm"

[plugins.openai.settings]
base-url = "https://api.example.com/v1"
egress-origin = "https://api.example.com"
model = "example-model"
"#,
        )
        .unwrap();

        let (id, plugin) = config.plugins().next().unwrap();
        assert_eq!(id, "openai");
        assert_eq!(
            plugin.component(),
            Path::new("./plugins/openai-compatible.wasm")
        );
        let settings = plugin.settings(id).unwrap();
        assert_eq!(settings["base-url"], "https://api.example.com/v1");
        assert_eq!(settings["egress-origin"], "https://api.example.com");
        assert_eq!(settings["model"], "example-model");
    }
}
