use crate::config::{Config, ConfiguredPlugin};
use crate::consent::{ConsentStore, PluginConsentReview, consent_drift};
use crate::session::{Session, SessionExecutor, SessionFuture, SessionManager, SessionOptions};
use crate::tool::ToolRegistry;
use crate::{ExecutionMode, Tool, ToolDefinition};
use lockgate::{
    ConsentRecord, ConsentRequired, Host, HostBuilder, InvocationCtx, PluginConfig, PluginHandle,
    Prepared, Role, RoleError, RuntimeLimits,
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
            drop(runtime.spawn_blocking(move || drop(resources)));
        } else {
            let _ = std::thread::Builder::new()
                .name("chap-lockgate-drop".to_owned())
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
    consent: ConsentStore,
    tools: ToolRegistry,
}

pub struct Agent {
    inner: Arc<AgentInner>,
}

#[derive(Clone, Copy)]
struct ToolExecutionConfig {
    mode: ExecutionMode,
    max_concurrency: usize,
}

pub(crate) struct AgentInner {
    lockgate: Arc<InnerHost>,
    plugins: BTreeMap<String, LoadedPlugin>,
    plugin_errors: BTreeMap<String, String>,
    sessions: SessionManager,
    tools: ToolRegistry,
    tool_execution: ToolExecutionConfig,
}

impl AgentBuilder {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let config = Config::load(path.as_ref())?;
        let consent = ConsentStore::new(config.consent_path());
        Ok(Self {
            config,
            consent,
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
        Ok(supported_roles(inspection.exported_interfaces()))
    }

    pub fn tool<T>(mut self, tool: T) -> Result<Self, String>
    where
        T: Tool + 'static,
    {
        self.tools.register(tool)?;
        Ok(self)
    }

    pub async fn approve_plugin(&self, id: &str) -> Result<ConsentRecord, String> {
        let (prepared, resources) = self.prepare_configured_plugin(id).await?;
        let record = prepared.approve(now_rfc3339());
        let save_result = self.consent.save(record.clone());
        let cleanup_result = Self::cleanup_prepared_plugin(id, prepared, resources).await;

        save_result?;
        cleanup_result?;
        Ok(record)
    }

    pub fn deny_plugin(&self, id: &str) -> Result<(), String> {
        if self.config.plugin(id).is_none() {
            return Err(format!("plugin `{id}` is not configured"));
        }
        self.consent.remove(id)
    }

    pub async fn review_plugin(&self, id: &str) -> Result<PluginConsentReview, String> {
        let (prepared, resources) = self.prepare_configured_plugin(id).await?;
        let manifest = prepared.review();
        let prior = self.consent.load(id);
        let drift = prior
            .as_ref()
            .filter(|prior| prior.request_digest != manifest.request_digest)
            .map(|prior| consent_drift(&prior.grants, &manifest.grants));
        let review = PluginConsentReview {
            manifest,
            prior,
            drift,
        };
        Self::cleanup_prepared_plugin(id, prepared, resources).await?;
        Ok(review)
    }

    async fn prepare_configured_plugin(
        &self,
        id: &str,
    ) -> Result<(Prepared, StartResources), String> {
        let plugin = self
            .config
            .plugin(id)
            .ok_or_else(|| format!("plugin `{id}` is not configured"))?;
        let builder = HostBuilder::new(())
            .map_err(|error| format!("failed to create Lockgate host: {error}"))?;
        let mut resources = StartResources::new(ToolRegistry::new(), builder);
        let prepared = match Self::prepare_plugin(
            resources.builder.as_mut().expect("uninitialized host"),
            &self.config,
            id,
            plugin,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                return match resources.cleanup().await {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(format!("{error}; {cleanup_error}")),
                };
            }
        };
        Ok((prepared, resources))
    }

    async fn cleanup_prepared_plugin(
        id: &str,
        prepared: Prepared,
        resources: StartResources,
    ) -> Result<(), String> {
        let prepared_cleanup = tokio::task::spawn_blocking(move || drop(prepared))
            .await
            .map_err(|error| format!("failed to clean up prepared plugin `{id}`: {error}"));
        let resource_cleanup = resources.cleanup().await;

        prepared_cleanup?;
        resource_cleanup?;
        Ok(())
    }

    pub async fn start(self) -> Result<Agent, String> {
        let Self {
            config,
            consent,
            tools,
        } = self;
        let builder = HostBuilder::new(())
            .map_err(|error| format!("failed to create Lockgate host: {error}"))?;
        let mut resources = StartResources::new(tools, builder);
        let (plugins, plugin_errors) =
            match Self::initialize_plugins(&mut resources, &config, &consent).await {
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
        let tool_execution = ToolExecutionConfig {
            mode: config.tools().execution(),
            max_concurrency: config.tools().max_concurrency().get(),
        };
        Ok(Agent {
            inner: Arc::new(AgentInner {
                lockgate,
                plugins,
                plugin_errors,
                sessions: SessionManager::new(),
                tools,
                tool_execution,
            }),
        })
    }

    async fn initialize_plugins(
        resources: &mut StartResources,
        config: &Config,
        consent: &ConsentStore,
    ) -> Result<(BTreeMap<String, LoadedPlugin>, BTreeMap<String, String>), String> {
        let (handles, plugin_errors) = Self::load_plugins(
            resources.builder.as_mut().expect("uninitialized host"),
            config,
            consent,
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
            for tool in PluginTool::load(
                id,
                Arc::clone(lockgate),
                plugin.handle.clone(),
                config.execution_mode(id),
            )
            .await?
            {
                resources
                    .tools
                    .as_mut()
                    .expect("initialized tool registry")
                    .register(tool)?;
            }
        }
        Ok((plugins, plugin_errors))
    }

    async fn load_plugins(
        builder: &mut HostBuilder<()>,
        config: &Config,
        consent: &ConsentStore,
    ) -> Result<(BTreeMap<String, PluginHandle>, BTreeMap<String, String>), String> {
        let mut plugins = BTreeMap::new();
        let mut plugin_errors = BTreeMap::new();

        for (id, plugin) in config.plugins() {
            match Self::load_plugin(builder, config, consent, id, plugin).await? {
                PluginLoad::Admitted(admitted) => {
                    plugins.insert(id.to_owned(), admitted);
                }
                PluginLoad::Refused(error) => {
                    plugin_errors.insert(id.to_owned(), error);
                }
            }
        }

        Ok((plugins, plugin_errors))
    }

    async fn load_plugin(
        builder: &mut HostBuilder<()>,
        config: &Config,
        consent: &ConsentStore,
        id: &str,
        plugin: &ConfiguredPlugin,
    ) -> Result<PluginLoad, String> {
        let path = config.component_path(plugin);
        let prepared = Self::prepare_plugin(builder, config, id, plugin).await?;
        let record = consent.load(id);
        let acceptance = match prepared.accept_reviewed(record.as_ref()) {
            Ok(acceptance) => acceptance,
            Err(required) => {
                let error = Self::consent_error(id, &path, required);
                tokio::task::spawn_blocking(move || drop(prepared))
                    .await
                    .map_err(|error| {
                        format!("failed to clean up refused plugin `{id}`: {error}")
                    })?;
                return Ok(PluginLoad::Refused(error));
            }
        };
        let refreshed_record = record.as_ref().and_then(|prior| {
            let manifest = prepared.review();
            (prior.request_digest != manifest.request_digest).then(|| ConsentRecord {
                instance_id: manifest.instance_id,
                request_digest: manifest.request_digest,
                component_digest: Some(manifest.component_digest),
                grants: manifest.grants,
                approved_at: prior.approved_at.clone(),
            })
        });
        let handle = builder
            .admit(
                prepared,
                acceptance,
                RuntimeLimits::default(),
                InvocationCtx::bounded(PLUGIN_FUEL_PER_CALL),
            )
            .await
            .map_err(|error| Self::load_error(id, &path, error))?;
        if let Some(record) = refreshed_record {
            consent.save(record)?;
        }
        Ok(PluginLoad::Admitted(handle))
    }

    async fn prepare_plugin(
        builder: &mut HostBuilder<()>,
        config: &Config,
        id: &str,
        plugin: &ConfiguredPlugin,
    ) -> Result<Prepared, String> {
        let path = config.component_path(plugin);
        let bytes = Self::plugin_bytes(config, id, plugin)?;
        let settings = plugin.settings();
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
        Self::validate_supported_role(id, &path, prepared.inspection().exported_interfaces())?;
        Ok(prepared)
    }

    fn consent_error(id: &str, path: &Path, required: ConsentRequired) -> String {
        match required {
            ConsentRequired::FirstRun { .. } => format!(
                "plugin `{id}` from `{}` requires approval before admission; run `chap grants review {id}` and then `chap grants approve {id}`",
                path.display()
            ),
            ConsentRequired::Drift { drift, .. } => {
                let change = if drift.blocks_admission {
                    "expanded its permission manifest"
                } else {
                    "reported a changed permission manifest"
                };
                format!(
                    "plugin `{id}` from `{}` {change} and requires renewed approval before admission; run `chap grants review {id}` and then `chap grants approve {id}`",
                    path.display()
                )
            }
        }
    }

    fn load_error(id: &str, path: &Path, error: impl std::fmt::Display) -> String {
        format!(
            "failed to load plugin `{id}` from `{}`: {error}",
            path.display()
        )
    }

    fn validate_supported_role(
        id: &str,
        path: &Path,
        exported_interfaces: &[String],
    ) -> Result<(), String> {
        if !supported_roles(exported_interfaces).is_empty() {
            return Ok(());
        }
        Err(format!(
            "plugin `{id}` from `{}` does not implement a supported role; expected an export from the `{}` package, but the component exports {}",
            path.display(),
            role_package(<bindings::provider::Role as Role>::INTERFACE),
            describe_exports(exported_interfaces),
        ))
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

fn supported_roles(interfaces: &[String]) -> Vec<&'static str> {
    let mut roles = Vec::new();
    if interfaces
        .iter()
        .any(|interface| interface == <bindings::provider::Role as Role>::INTERFACE)
    {
        roles.push("provider");
    }
    if interfaces
        .iter()
        .any(|interface| interface == <bindings::tools::Role as Role>::INTERFACE)
    {
        roles.push("tool");
    }
    roles
}

fn role_package(interface: &str) -> String {
    let Some((package, rest)) = interface.split_once('/') else {
        return interface.to_owned();
    };
    match rest.split_once('@') {
        Some((_, version)) => format!("{package}@{version}"),
        None => package.to_owned(),
    }
}

fn describe_exports(interfaces: &[String]) -> String {
    if interfaces.is_empty() {
        return "no interfaces".to_owned();
    }
    interfaces
        .iter()
        .map(|interface| format!("`{interface}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("the current UTC time must be representable as RFC3339")
}

impl Agent {
    pub fn plugin_errors(&self) -> impl Iterator<Item = (&str, &str)> {
        self.inner
            .plugin_errors
            .iter()
            .map(|(id, error)| (id.as_str(), error.as_str()))
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.inner.tools.definitions()
    }

    pub fn session(&self, options: SessionOptions) -> Result<Session, String> {
        if let Some(error) = self.inner.plugin_errors.get(&options.provider) {
            return Err(error.clone());
        }
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

#[allow(clippy::large_enum_variant)]
enum PluginLoad {
    Admitted(PluginHandle),
    Refused(String),
}

impl AgentInner {
    async fn run_turn(&self, session: &Session, input: String) -> Result<String, String> {
        if !self.sessions.owns(&session.state) {
            return Err("session does not belong to this runtime".to_owned());
        }
        let backend = PluginBackend::new(self, session.provider());
        run_agent_loop(
            &session.state,
            input,
            &self.tools,
            self.tool_execution,
            &backend,
        )
        .await
    }
}

impl SessionExecutor for AgentInner {
    fn send<'a>(&'a self, session: &'a Session, input: String) -> SessionFuture<'a> {
        Box::pin(self.run_turn(session, input))
    }
}

#[cfg(test)]
mod tests;
