//! Namespaced plugin settings and environment-backed secret resolution.

use super::AppState;
use crate::{agent::bindings, config::Config};
use lockgate::HostContext;
use std::{collections::BTreeMap, env};

pub(super) struct Settings {
    values: BTreeMap<String, String>,
}

impl Settings {
    pub(super) fn from_config(config: &Config) -> Result<Self, String> {
        let values = config
            .plugins()
            .map(|(id, plugin)| {
                let mut settings = plugin.settings().clone();
                let api_key_env = settings.remove("api-key-env");
                if !settings.contains_key("api-key") && let Some(variable) = api_key_env {
                    let variable = variable.as_str().ok_or_else(|| {
                        format!("plugin `{id}` setting `api-key-env` must be a string")
                    })?;
                    let api_key = env::var(variable).map_err(|error| {
                        format!(
                            "failed to read API key for plugin `{id}` from environment variable `{variable}`: {error}"
                        )
                    })?;
                    settings.insert("api-key".to_owned(), toml::Value::String(api_key));
                }
                let json = serde_json::to_string(&settings)
                    .map_err(|error| format!("failed to encode settings for plugin `{id}`: {error}"))?;
                Ok((id.to_owned(), json))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { values })
    }

    pub(super) fn get_json(&self, plugin_id: &str) -> Result<String, String> {
        self.values
            .get(plugin_id)
            .cloned()
            .ok_or_else(|| format!("settings are unavailable for plugin `{plugin_id}`"))
    }
}

impl bindings::sage::agent::settings::Host for HostContext<AppState> {}

impl bindings::sage::agent::settings::HostWithStore<lockgate::__private::PluginStore<AppState>>
    for HostContext<AppState>
{
    async fn get_json(
        accessor: &wasmtime::component::Accessor<lockgate::__private::PluginStore<AppState>, Self>,
    ) -> Result<String, String> {
        accessor.with(|mut access| {
            let context = access.get();
            context.state().settings.get_json(context.plugin().id())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_toml_value_types_and_nested_settings() {
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

        let settings = Settings::from_config(&config).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&settings.get_json("example").unwrap()).unwrap();

        assert_eq!(
            json,
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
    fn resolves_api_key_from_the_environment_and_removes_the_selector() {
        let config: Config = toml::from_str(
            r#"
[plugins.example]
component = "example.wasm"

[plugins.example.settings]
api-key-env = "PATH"
"#,
        )
        .unwrap();

        let settings = Settings::from_config(&config).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&settings.get_json("example").unwrap()).unwrap();

        assert!(json["api-key"].is_string());
        assert!(json.get("api-key-env").is_none());
    }

    #[test]
    fn explicit_api_key_takes_precedence_and_removes_the_selector() {
        let config: Config = toml::from_str(
            r#"
[plugins.example]
component = "example.wasm"

[plugins.example.settings]
api-key = "configured"
api-key-env = 42
"#,
        )
        .unwrap();

        let settings = Settings::from_config(&config).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&settings.get_json("example").unwrap()).unwrap();

        assert!(json["api-key"].is_string());
        assert!(json.get("api-key-env").is_none());
    }
}
