use crate::config::{Config, Plugin as PluginConfig};
use crate::session::{Session, SessionExecutor, SessionFuture, SessionManager, SessionOptions};
use crate::tool::ToolRegistry;
use crate::{Tool, ToolDefinition};
use host::{AppState, SETTINGS_INTERFACE};
use lockgate::Component;
use plugin_tool::PluginTool;
use provider::PluginBackend;
use std::{collections::BTreeMap, fs, path::Path, sync::Arc};
use turn::run_agent_loop;

mod bindings;
mod host;
mod plugin_tool;
mod provider;
mod turn;

const PLUGIN_FUEL_PER_CALL: u64 = 25_000_000;
const MAX_PROVIDER_STEPS_PER_TURN: usize = 64;

type InnerApplication = lockgate::Application<AppState>;
type InnerRuntime = lockgate::Runtime<AppState>;
type ProviderComponent = Component<bindings::ProviderPlugin>;
type ToolComponent = Component<bindings::ToolPlugin>;

#[derive(Clone, Copy)]
struct LoadedPlugin {
    provider: Option<ProviderComponent>,
    tools: Option<ToolComponent>,
}

pub struct AgentBuilder {
    config: Config,
    tools: ToolRegistry,
}

pub struct Agent {
    inner: Arc<AgentInner>,
}

pub(crate) struct AgentInner {
    lockgate: Arc<InnerRuntime>,
    plugins: BTreeMap<String, LoadedPlugin>,
    sessions: SessionManager,
    tools: ToolRegistry,
}

impl AgentBuilder {
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

    pub fn plugin_roles(&self, id: &str) -> Result<Vec<&'static str>, String> {
        let plugin = self
            .config
            .plugin(id)
            .ok_or_else(|| format!("plugin `{id}` is not configured"))?;
        let lockgate = self.lockgate()?;
        let bytes = Self::plugin_bytes(&self.config, id, plugin)?;
        let mut roles = Vec::new();
        if lockgate
            .supports::<bindings::ProviderPlugin>(&bytes)
            .map_err(|error| format!("failed to inspect plugin `{id}`: {error}"))?
        {
            roles.push("provider");
        }
        if lockgate
            .supports::<bindings::ToolPlugin>(&bytes)
            .map_err(|error| format!("failed to inspect plugin `{id}`: {error}"))?
        {
            roles.push("tool");
        }
        Ok(roles)
    }

    pub fn tool<T>(mut self, tool: T) -> Result<Self, String>
    where
        T: Tool + 'static,
    {
        self.tools.register(tool)?;
        Ok(self)
    }

    pub async fn start(self) -> Result<Agent, String> {
        let mut lockgate = self.lockgate()?;
        let plugins = Self::load_plugins(&mut lockgate, &self.config)?;
        let lockgate = Self::apply_policy(lockgate, &self.config, &plugins)?;
        let lockgate = Arc::new(
            lockgate
                .run()
                .await
                .map_err(|error| format!("failed to start plugin runtime: {error}"))?,
        );
        let mut tools = self.tools;
        for (id, plugin) in &plugins {
            let Some(component) = plugin.tools else {
                continue;
            };
            for tool in PluginTool::load(id, Arc::clone(&lockgate), component).await? {
                tools.register(tool)?;
            }
        }
        Ok(Agent {
            inner: Arc::new(AgentInner {
                lockgate,
                plugins,
                sessions: SessionManager::new(),
                tools,
            }),
        })
    }

    fn lockgate(&self) -> Result<InnerApplication, String> {
        lockgate::Application::new(AppState::from_config(&self.config)?)
            .map_err(|error| format!("failed to create Lockgate application: {error}"))
            .map(|lockgate| lockgate.fuel_per_call(PLUGIN_FUEL_PER_CALL))
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
        let bytes = Self::plugin_bytes(config, id, plugin)?;
        let provider = lockgate
            .supports::<bindings::ProviderPlugin>(&bytes)
            .map_err(|error| format!("failed to inspect plugin `{id}`: {error}"))?;
        let tools = lockgate
            .supports::<bindings::ToolPlugin>(&bytes)
            .map_err(|error| format!("failed to inspect plugin `{id}`: {error}"))?;
        let loaded = match (provider, tools) {
            (true, true) => {
                let (provider, tools) = lockgate
                    .add::<(bindings::ProviderPlugin, bindings::ToolPlugin)>(bytes)
                    .map_err(|error| Self::load_error(id, &path, error))?;
                LoadedPlugin {
                    provider: Some(provider),
                    tools: Some(tools),
                }
            }
            (true, false) => LoadedPlugin {
                provider: Some(
                    lockgate
                        .add::<bindings::ProviderPlugin>(bytes)
                        .map_err(|error| Self::load_error(id, &path, error))?,
                ),
                tools: None,
            },
            (false, true) => LoadedPlugin {
                provider: None,
                tools: Some(
                    lockgate
                        .add::<bindings::ToolPlugin>(bytes)
                        .map_err(|error| Self::load_error(id, &path, error))?,
                ),
            },
            (false, false) => {
                return Err(format!(
                    "plugin `{id}` from `{}` does not implement a supported role",
                    path.display()
                ));
            }
        };

        if let Some(component) = loaded.provider {
            Self::validate_plugin_id(lockgate, component, id, &path)?;
        } else if let Some(component) = loaded.tools {
            Self::validate_plugin_id(lockgate, component, id, &path)?;
        }
        Ok(loaded)
    }

    fn load_error(id: &str, path: &Path, error: impl std::fmt::Display) -> String {
        format!(
            "failed to load plugin `{id}` from `{}`: {error}",
            path.display()
        )
    }

    fn plugin_bytes(config: &Config, id: &str, plugin: &PluginConfig) -> Result<Vec<u8>, String> {
        let path = config.component_path(plugin);
        fs::read(&path).map_err(|error| {
            format!(
                "failed to read plugin `{id}` from `{}`: {error}",
                path.display()
            )
        })
    }

    fn validate_plugin_id<B>(
        lockgate: &InnerApplication,
        plugin: Component<B>,
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
        config: &Config,
        plugins: &BTreeMap<String, LoadedPlugin>,
    ) -> Result<InnerApplication, String> {
        // TODO: Let Lockgate configure and enforce URL-scoped WASI HTTP policies instead of
        // granting unrestricted outbound HTTP.
        for (id, plugin) in plugins {
            let configured = config
                .plugin(id)
                .expect("loaded plugins come from the configuration");
            lockgate = if let Some(component) = plugin.provider {
                Self::apply_component_policy(lockgate, configured, id, component)?
            } else if let Some(component) = plugin.tools {
                Self::apply_component_policy(lockgate, configured, id, component)?
            } else {
                unreachable!("loaded plugins always implement at least one role")
            };
        }

        Ok(lockgate)
    }

    fn apply_component_policy<B>(
        mut lockgate: InnerApplication,
        configured: &PluginConfig,
        id: &str,
        component: Component<B>,
    ) -> Result<InnerApplication, String> {
        if configured.outbound_http() {
            lockgate = lockgate
                .allow_outbound_http(component)
                .map_err(|error| format!("failed to configure plugin `{id}`: {error}"))?;
        }
        lockgate = lockgate
            .allow_host_import(component, SETTINGS_INTERFACE)
            .map_err(|error| format!("failed to configure plugin `{id}`: {error}"))?;
        Ok(lockgate)
    }
}

impl Agent {
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.inner.tools.definitions()
    }

    pub fn session(&self, options: SessionOptions) -> Result<Session, String> {
        if !self
            .inner
            .plugins
            .get(&options.provider)
            .is_some_and(|plugin| plugin.provider.is_some())
        {
            return Err(format!(
                "provider plugin `{}` is not configured",
                options.provider
            ));
        }
        let state = self.inner.sessions.create(options)?;
        Ok(Session::new(state, self.inner.clone()))
    }
}

impl AgentInner {
    async fn run_turn(&self, session: &Session, input: String) -> Result<String, String> {
        if !self.sessions.owns(&session.state) {
            return Err("session does not belong to this runtime".to_owned());
        }
        let backend = PluginBackend::new(self, session.provider());
        run_agent_loop(&session.state, input, &self.tools, &backend).await
    }
}

impl SessionExecutor for AgentInner {
    fn send<'a>(&'a self, session: &'a Session, input: String) -> SessionFuture<'a> {
        Box::pin(self.run_turn(session, input))
    }
}

#[cfg(test)]
mod tests;
