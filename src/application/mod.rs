use crate::config::{Config, Plugin as PluginConfig};
use host::{AppState, SETTINGS_INTERFACE};
use lockgate::Component;
use std::{collections::BTreeMap, fs, path::Path};

mod bindings;
mod host;

const PLUGIN_FUEL_PER_CALL: u64 = 25_000_000;

type InnerApplication = lockgate::Application<AppState>;
type LoadedPlugin = Component<bindings::ProviderPlugin>;

pub(crate) struct Application {
    _lockgate: InnerApplication,
    plugins: BTreeMap<String, LoadedPlugin>,
}

impl Application {
    pub(crate) fn load(config: &Config) -> Result<Self, String> {
        let mut lockgate = lockgate::Application::new(AppState::from_config(config))
            .map_err(|error| format!("failed to create Lockgate application: {error}"))?
            .fuel_per_call(PLUGIN_FUEL_PER_CALL);
        let plugins = Self::load_plugins(&mut lockgate, config)?;
        let lockgate = Self::apply_policy(lockgate, &plugins)?;

        Ok(Self {
            _lockgate: lockgate,
            plugins,
        })
    }

    pub(crate) fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    fn load_plugins(
        lockgate: &mut InnerApplication,
        config: &Config,
    ) -> Result<BTreeMap<String, LoadedPlugin>, String> {
        let mut plugins = BTreeMap::new();

        for (id, plugin) in config.plugins() {
            let loaded = Self::load_plugin(lockgate, config, id, plugin)?;
            plugins.insert(id.to_owned(), loaded);
        }

        Ok(plugins)
    }

    fn load_plugin(
        lockgate: &mut InnerApplication,
        config: &Config,
        id: &str,
        plugin: &PluginConfig,
    ) -> Result<LoadedPlugin, String> {
        let path = config.component_path(plugin);
        let bytes = fs::read(&path).map_err(|error| {
            format!(
                "failed to read plugin `{id}` from `{}`: {error}",
                path.display()
            )
        })?;
        let loaded = lockgate
            .add::<bindings::ProviderPlugin>(bytes)
            .map_err(|error| {
                format!(
                    "failed to load plugin `{id}` from `{}`: {error}",
                    path.display()
                )
            })?;

        Self::validate_plugin_id(lockgate, loaded, id, &path)?;
        Ok(loaded)
    }

    fn validate_plugin_id(
        lockgate: &InnerApplication,
        plugin: LoadedPlugin,
        id: &str,
        path: &Path,
    ) -> Result<(), String> {
        let embedded_id = lockgate
            .metadata(plugin)
            .map_err(|error| format!("failed to inspect plugin `{id}`: {error}"))?
            .id();
        if embedded_id != id {
            return Err(format!(
                "plugin `{id}` declares embedded id `{embedded_id}` in `{}`",
                path.display()
            ));
        }
        Ok(())
    }

    fn apply_policy(
        mut lockgate: InnerApplication,
        plugins: &BTreeMap<String, LoadedPlugin>,
    ) -> Result<InnerApplication, String> {
        // TODO: Load per-plugin capability grants from `sage.toml` or a separate policy document
        // instead of granting every plugin access to settings.
        for (id, plugin) in plugins {
            lockgate = lockgate
                .allow_host_import(*plugin, SETTINGS_INTERFACE)
                .map_err(|error| format!("failed to configure plugin `{id}`: {error}"))?;
        }

        Ok(lockgate)
    }
}

#[cfg(test)]
mod tests;
