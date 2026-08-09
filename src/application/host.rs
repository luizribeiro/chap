use super::bindings;
use crate::config::Config;
use lockgate::HostContext;
use std::collections::BTreeMap;

pub(super) const SETTINGS_INTERFACE: &str = "sage:agent/settings@0.1.0";

pub(super) struct AppState {
    settings: BTreeMap<String, toml::Table>,
}

impl AppState {
    pub(super) fn from_config(config: &Config) -> Self {
        Self {
            settings: config
                .plugins()
                .map(|(id, plugin)| (id.to_owned(), plugin.settings().clone()))
                .collect(),
        }
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
            context
                .state()
                .settings
                .get(context.plugin().id())
                .and_then(|settings| settings.get(&key))
                .map(setting_value)
        })
    }
}

fn setting_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}
