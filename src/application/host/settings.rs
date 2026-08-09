use super::AppState;
use crate::{application::bindings, config::Config};
use lockgate::HostContext;
use std::{collections::BTreeMap, env};

pub(super) struct Settings {
    values: BTreeMap<String, toml::Table>,
}

impl Settings {
    pub(super) fn from_config(config: &Config) -> Result<Self, String> {
        let values = config
            .plugins()
            .map(|(id, plugin)| {
                let mut settings = plugin.settings().clone();
                if !settings.contains_key("api-key")
                    && let Some(variable) = settings.get("api-key-env")
                {
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
                Ok((id.to_owned(), settings))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { values })
    }

    pub(super) fn get(&self, plugin_id: &str, key: &str) -> Option<String> {
        self.values.get(plugin_id)?.get(key).map(setting_value)
    }
}

impl bindings::sage::agent::settings::Host for HostContext<AppState> {}

impl bindings::sage::agent::settings::HostWithStore<lockgate::__private::PluginStore<AppState>>
    for HostContext<AppState>
{
    async fn get(
        accessor: &wasmtime::component::Accessor<lockgate::__private::PluginStore<AppState>, Self>,
        key: String,
    ) -> Option<String> {
        accessor.with(|mut access| {
            let context = access.get();
            context.state().settings.get(context.plugin().id(), &key)
        })
    }
}

fn setting_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}
