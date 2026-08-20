use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    plugins: BTreeMap<String, ConfiguredPlugin>,
    #[serde(skip)]
    directory: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredPlugin {
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
        toml_to_json(toml::Value::Table(self.settings.clone()))
    }
}

fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(value) => Value::String(value),
        toml::Value::Integer(value) => Value::Number(value.into()),
        toml::Value::Float(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(toml::Value::Float(value).to_string())),
        toml::Value::Boolean(value) => Value::Bool(value),
        toml::Value::Datetime(value) => Value::String(value.to_string()),
        toml::Value::Array(values) => Value::Array(values.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, toml_to_json(value)))
                .collect(),
        ),
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
        let settings = plugin.settings();
        assert_eq!(settings["base-url"], "https://api.example.com/v1");
        assert_eq!(settings["model"], "example-model");
    }

    #[test]
    fn preserves_toml_types_and_nested_settings() {
        let config: Config = toml::from_str(
            r#"
[plugins.example]
component = "example.wasm"

[plugins.example.settings]
string = "value"
integer = 42
float = 1.5
boolean = true
array = ["one", "two"]

[plugins.example.settings.nested]
enabled = false
"#,
        )
        .unwrap();

        let settings = config.plugin("example").unwrap().settings();
        assert_eq!(
            settings,
            serde_json::json!({
                "string": "value",
                "integer": 42,
                "float": 1.5,
                "boolean": true,
                "array": ["one", "two"],
                "nested": {"enabled": false}
            })
        );
    }

    #[test]
    fn converts_toml_only_values_to_plain_json_strings() {
        let config: Config = toml::from_str(
            r#"
[plugins.example]
component = "example.wasm"

[plugins.example.settings]
date = 1979-05-27
time = 07:32:00
local-date-time = 1979-05-27T07:32:00
offset-date-time = 1979-05-27T07:32:00Z
not-a-number = nan
infinity = inf
dates = [1979-05-27, 1980-05-27]
"#,
        )
        .unwrap();

        let settings = config.plugin("example").unwrap().settings();
        assert_eq!(settings["date"], "1979-05-27");
        assert_eq!(settings["time"], "07:32:00");
        assert_eq!(settings["local-date-time"], "1979-05-27T07:32:00");
        assert_eq!(settings["offset-date-time"], "1979-05-27T07:32:00Z");
        assert_eq!(settings["not-a-number"], "nan");
        assert_eq!(settings["infinity"], "inf");
        assert_eq!(
            settings["dates"],
            serde_json::json!(["1979-05-27", "1980-05-27"])
        );
    }

    #[test]
    fn passes_api_key_env_through_untouched() {
        let config: Config = toml::from_str(
            r#"
[plugins.example]
component = "example.wasm"

[plugins.example.settings]
api-key-env = "EXAMPLE_API_KEY"
"#,
        )
        .unwrap();

        let settings = config.plugin("example").unwrap().settings();
        assert_eq!(settings["api-key-env"], "EXAMPLE_API_KEY");
        assert!(settings.get("api-key").is_none());
    }
}
