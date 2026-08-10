use crate::config::{Config, Plugin as PluginConfig};
use crate::session::{Session, SessionExecutor, SessionFuture, SessionManager, SessionOptions};
use crate::tool::ToolRegistry;
use crate::{Tool, ToolDefinition};
use host::{AppState, HTTP_CLIENT_INTERFACE, SETTINGS_INTERFACE};
use lockgate::Component;
use provider::PluginBackend;
use std::{collections::BTreeMap, fs, path::Path, sync::Arc};
use turn::run_agent_loop;

mod bindings;
mod host;
mod provider;
mod turn;

const PLUGIN_FUEL_PER_CALL: u64 = 25_000_000;
const MAX_PROVIDER_STEPS_PER_TURN: usize = 64;

type InnerApplication = lockgate::Application<AppState>;
type InnerRuntime = lockgate::Runtime<AppState>;
type LoadedPlugin = Component<bindings::ProviderPlugin>;

pub struct SageBuilder {
    config: Config,
    tools: ToolRegistry,
}

pub struct Sage {
    inner: Arc<SageInner>,
}

pub(crate) struct SageInner {
    lockgate: InnerRuntime,
    plugins: BTreeMap<String, LoadedPlugin>,
    sessions: SessionManager,
    tools: ToolRegistry,
}

impl SageBuilder {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        Ok(Self {
            config: Config::load(path.as_ref())?,
            tools: ToolRegistry::new(),
        })
    }

    pub fn plugins(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.config
            .plugins()
            .map(|(id, plugin)| (id, plugin.component()))
    }

    pub fn tool<T>(mut self, tool: T) -> Result<Self, String>
    where
        T: Tool + 'static,
    {
        self.tools.register(tool)?;
        Ok(self)
    }

    pub async fn start(self) -> Result<Sage, String> {
        let mut lockgate = lockgate::Application::new(AppState::from_config(&self.config)?)
            .map_err(|error| format!("failed to create Lockgate application: {error}"))?
            .fuel_per_call(PLUGIN_FUEL_PER_CALL);
        let plugins = Self::load_plugins(&mut lockgate, &self.config)?;
        let lockgate = Self::apply_policy(lockgate, &plugins)?;
        let lockgate = lockgate
            .run()
            .await
            .map_err(|error| format!("failed to start plugin runtime: {error}"))?;
        Ok(Sage {
            inner: Arc::new(SageInner {
                lockgate,
                plugins,
                sessions: SessionManager::new(),
                tools: self.tools,
            }),
        })
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
        // TODO: Move capability policy into Lockgate configuration instead of granting every
        // plugin the same host imports. HTTP should use `wasi:http`, with Lockgate granting and
        // enforcing per-plugin URL restrictions.
        for (id, plugin) in plugins {
            lockgate = lockgate
                .allow_host_import(*plugin, HTTP_CLIENT_INTERFACE)
                .map_err(|error| format!("failed to configure plugin `{id}`: {error}"))?
                .allow_host_import(*plugin, SETTINGS_INTERFACE)
                .map_err(|error| format!("failed to configure plugin `{id}`: {error}"))?;
        }

        Ok(lockgate)
    }
}

impl Sage {
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.inner.tools.definitions()
    }

    pub fn session(&self, options: SessionOptions) -> Result<Session, String> {
        if !self.inner.plugins.contains_key(&options.provider) {
            return Err(format!(
                "provider plugin `{}` is not configured",
                options.provider
            ));
        }
        let state = self.inner.sessions.create(options)?;
        Ok(Session::new(state, self.inner.clone()))
    }
}

impl SageInner {
    async fn run_turn(&self, session: &Session, input: String) -> Result<String, String> {
        if !self.sessions.owns(&session.state) {
            return Err("session does not belong to this runtime".to_owned());
        }
        let backend = PluginBackend::new(self, session.provider());
        run_agent_loop(&session.state, input, &self.tools, &backend).await
    }
}

impl SessionExecutor for SageInner {
    fn send<'a>(&'a self, session: &'a Session, input: String) -> SessionFuture<'a> {
        Box::pin(self.run_turn(session, input))
    }
}

#[cfg(test)]
mod tests;
