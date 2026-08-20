use crate::config::{Config, Plugin as ConfiguredPlugin};
use crate::session::{Session, SessionExecutor, SessionFuture, SessionManager, SessionOptions};
use crate::tool::ToolRegistry;
use crate::{Tool, ToolDefinition};
use lockgate::{
    Host, HostBuilder, InvocationCtx, PluginConfig, PluginHandle, Role, RoleError, RuntimeLimits,
};
use plugin_tool::PluginTool;
use provider::PluginBackend;
use std::{collections::BTreeMap, fs, path::Path, sync::Arc};
use turn::run_agent_loop;

mod bindings;
mod plugin_tool;
mod provider;
mod turn;

const PLUGIN_FUEL_PER_CALL: u64 = 25_000_000;
const MAX_PROVIDER_STEPS_PER_TURN: usize = 64;

type InnerHost = Host<()>;
type StartDropResources = (
    Option<ToolRegistry>,
    Option<Arc<InnerHost>>,
    Option<HostBuilder<()>>,
);

struct StartResources {
    tools: Option<ToolRegistry>,
    host: Option<Arc<InnerHost>>,
    builder: Option<HostBuilder<()>>,
}

impl StartResources {
    fn new(tools: ToolRegistry, builder: HostBuilder<()>) -> Self {
        Self {
            tools: Some(tools),
            host: None,
            builder: Some(builder),
        }
    }

    fn take_drop_resources(&mut self) -> StartDropResources {
        (self.tools.take(), self.host.take(), self.builder.take())
    }

    async fn cleanup(mut self) -> Result<(), String> {
        let resources = self.take_drop_resources();
        tokio::task::spawn_blocking(move || drop(resources))
            .await
            .map_err(|error| format!("failed to clean up Lockgate host: {error}"))
    }
}

impl Drop for StartResources {
    fn drop(&mut self) {
        let resources = self.take_drop_resources();
        if resources.0.is_none() && resources.1.is_none() && resources.2.is_none() {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let _ = runtime.spawn_blocking(move || drop(resources));
        } else {
            let _ = std::thread::Builder::new()
                .name("sage-lockgate-drop".to_owned())
                .spawn(move || drop(resources));
        }
    }
}

#[derive(Clone)]
struct LoadedPlugin {
    handle: PluginHandle,
    provider: bool,
    tools: bool,
}

pub struct AgentBuilder {
    config: Config,
    tools: ToolRegistry,
}

pub struct Agent {
    inner: Arc<AgentInner>,
}

pub(crate) struct AgentInner {
    lockgate: Arc<InnerHost>,
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
        let bytes = Self::plugin_bytes(&self.config, id, plugin)?;
        let inspection = lockgate::inspect(&bytes)
            .map_err(|error| format!("failed to inspect plugin `{id}`: {error}"))?;
        let mut roles = Vec::new();
        if inspection
            .exported_interfaces()
            .iter()
            .any(|interface| interface == <bindings::provider::Role as Role>::INTERFACE)
        {
            roles.push("provider");
        }
        if inspection
            .exported_interfaces()
            .iter()
            .any(|interface| interface == <bindings::tools::Role as Role>::INTERFACE)
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
        let Self { config, tools } = self;
        let builder = HostBuilder::new(())
            .map_err(|error| format!("failed to create Lockgate host: {error}"))?;
        let mut resources = StartResources::new(tools, builder);
        let plugins = match Self::initialize_plugins(&mut resources, &config).await {
            Ok(plugins) => plugins,
            Err(error) => {
                return match resources.cleanup().await {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(format!("{error}; {cleanup_error}")),
                };
            }
        };
        let lockgate = resources.host.take().expect("initialized Lockgate host");
        let tools = resources.tools.take().expect("initialized tool registry");
        Ok(Agent {
            inner: Arc::new(AgentInner {
                lockgate,
                plugins,
                sessions: SessionManager::new(),
                tools,
            }),
        })
    }

    async fn initialize_plugins(
        resources: &mut StartResources,
        config: &Config,
    ) -> Result<BTreeMap<String, LoadedPlugin>, String> {
        let handles = Self::load_plugins(
            resources.builder.as_mut().expect("uninitialized host"),
            config,
        )
        .await?;
        let builder = resources.builder.take().expect("uninitialized host");
        resources.host = Some(Arc::new(builder.finish()));
        let lockgate = resources.host.as_ref().expect("initialized host");
        let plugins = Self::classify_plugins(lockgate, handles)?;
        for (id, plugin) in &plugins {
            if !plugin.tools {
                continue;
            }
            for tool in PluginTool::load(id, Arc::clone(&lockgate), plugin.handle.clone()).await? {
                resources
                    .tools
                    .as_mut()
                    .expect("initialized tool registry")
                    .register(tool)?;
            }
        }
        Ok(plugins)
    }

    async fn load_plugins(
        builder: &mut HostBuilder<()>,
        config: &Config,
    ) -> Result<BTreeMap<String, PluginHandle>, String> {
        let mut plugins = BTreeMap::new();

        for (id, plugin) in config.plugins() {
            let handle = Self::load_plugin(builder, config, id, plugin).await?;
            plugins.insert(id.to_owned(), handle);
        }

        Ok(plugins)
    }

    async fn load_plugin(
        builder: &mut HostBuilder<()>,
        config: &Config,
        id: &str,
        plugin: &ConfiguredPlugin,
    ) -> Result<PluginHandle, String> {
        let path = config.component_path(plugin);
        let bytes = Self::plugin_bytes(config, id, plugin)?;
        let settings = plugin.settings(id)?;
        let prepared = builder
            .prepare(
                id,
                &bytes,
                PluginConfig {
                    settings: Some(settings),
                    ..PluginConfig::default()
                },
            )
            .await
            .map_err(|error| Self::load_error(id, &path, error))?;
        let acceptance = prepared.accept_all();
        builder
            .admit(
                prepared,
                acceptance,
                RuntimeLimits::default(),
                InvocationCtx::bounded(PLUGIN_FUEL_PER_CALL),
            )
            .await
            .map_err(|error| Self::load_error(id, &path, error))
    }

    fn load_error(id: &str, path: &Path, error: impl std::fmt::Display) -> String {
        format!(
            "failed to load plugin `{id}` from `{}`: {error}",
            path.display()
        )
    }

    fn plugin_bytes(
        config: &Config,
        id: &str,
        plugin: &ConfiguredPlugin,
    ) -> Result<Vec<u8>, String> {
        let path = config.component_path(plugin);
        fs::read(&path).map_err(|error| {
            format!(
                "failed to read plugin `{id}` from `{}`: {error}",
                path.display()
            )
        })
    }

    fn classify_plugins(
        host: &InnerHost,
        handles: BTreeMap<String, PluginHandle>,
    ) -> Result<BTreeMap<String, LoadedPlugin>, String> {
        handles
            .into_iter()
            .map(|(id, handle)| {
                let provider = Self::exports_role::<bindings::provider::Role>(host, &handle, &id)?;
                let tools = Self::exports_role::<bindings::tools::Role>(host, &handle, &id)?;
                if !provider && !tools {
                    return Err(format!("plugin `{id}` does not implement a supported role"));
                }
                Ok((
                    id,
                    LoadedPlugin {
                        handle,
                        provider,
                        tools,
                    },
                ))
            })
            .collect()
    }

    fn exports_role<R: Role>(
        host: &InnerHost,
        handle: &PluginHandle,
        id: &str,
    ) -> Result<bool, String> {
        match host.client::<R>(handle) {
            Ok(_) => Ok(true),
            Err(RoleError::RoleNotExported { .. }) => Ok(false),
            Err(error) => Err(format!(
                "failed to inspect roles for plugin `{id}`: {error}"
            )),
        }
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
            .is_some_and(|plugin| plugin.provider)
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
