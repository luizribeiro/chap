use crate::config::{Config, ConfiguredPlugin, ContextChannel};
use crate::consent::{ConsentStore, PluginConsentReview, consent_drift};
use crate::session::{
    RunError, Session, SessionExecutor, SessionFuture, SessionManager, SessionOptions,
};
use crate::tool::ToolRegistry;
use crate::{ExecutionMode, Tool, ToolDefinition};
use lockgate::{
    ConsentRecord, ConsentRequired, Host, HostBuilder, InvocationCtx, PluginConfig, PluginHandle,
    Prepared, Role, RuntimeLimits,
};
use plugin_tool::PluginTool;
use provider::PluginBackend;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex as AsyncMutex;
use turn::run_agent_loop;

mod bindings;
mod context;
mod plugin_tool;
mod provider;
mod turn;

const PLUGIN_FUEL_PER_CALL: u64 = 25_000_000;
const PLUGIN_ADMISSION_DEADLINE: Duration = Duration::from_secs(30);
const CONTEXT_ASSEMBLY_DEADLINE: Duration = Duration::from_secs(10);
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
    roles: BTreeSet<&'static str>,
    context_channel: ContextChannel,
}

struct AdmittedPlugin {
    handle: PluginHandle,
    exported_interfaces: Vec<String>,
    context_channel: ContextChannel,
}

impl LoadedPlugin {
    fn has_role(&self, role: &chap_wit::Role) -> bool {
        self.roles.contains(role.interface)
    }
}

pub struct AgentBuilder {
    config: Config,
    consent: ConsentStore,
    tools: ToolRegistry,
    plugin_call_deadlines: PluginCallDeadlines,
}

pub struct Agent {
    inner: Arc<AgentInner>,
}

#[derive(Clone, Copy)]
struct ToolExecutionConfig {
    mode: ExecutionMode,
    max_concurrency: usize,
}

#[derive(Clone, Copy)]
struct PluginCallDeadlines {
    provider: Duration,
    tool: Duration,
}

pub(crate) struct AgentInner {
    lockgate: Arc<InnerHost>,
    plugins: BTreeMap<String, LoadedPlugin>,
    plugin_errors: BTreeMap<String, String>,
    sessions: SessionManager,
    tools: ToolRegistry,
    tool_execution: ToolExecutionConfig,
    plugin_call_deadlines: PluginCallDeadlines,
    context_last_good:
        AsyncMutex<BTreeMap<(crate::SessionId, String), Vec<context::ContextSegment>>>,
}

impl AgentBuilder {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let config = Config::load(path.as_ref())?;
        let consent = ConsentStore::new(config.consent_path());
        let plugin_call_deadlines = PluginCallDeadlines {
            provider: config.provider().deadline(),
            tool: config.tools().deadline(),
        };
        Ok(Self {
            config,
            consent,
            tools: ToolRegistry::new(),
            plugin_call_deadlines,
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
            plugin_call_deadlines,
        } = self;
        let builder = HostBuilder::new(())
            .map_err(|error| format!("failed to create Lockgate host: {error}"))?;
        let mut resources = StartResources::new(tools, builder);
        let (plugins, plugin_errors) = match Self::initialize_plugins(
            &mut resources,
            &config,
            &consent,
            plugin_call_deadlines,
        )
        .await
        {
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
                plugin_call_deadlines,
                context_last_good: AsyncMutex::new(BTreeMap::new()),
            }),
        })
    }

    async fn initialize_plugins(
        resources: &mut StartResources,
        config: &Config,
        consent: &ConsentStore,
        plugin_call_deadlines: PluginCallDeadlines,
    ) -> Result<(BTreeMap<String, LoadedPlugin>, BTreeMap<String, String>), String> {
        let (admitted, plugin_errors) = Self::load_plugins(
            resources.builder.as_mut().expect("uninitialized host"),
            config,
            consent,
            plugin_call_deadlines,
        )
        .await?;
        let builder = resources.builder.take().expect("uninitialized host");
        resources.host = Some(Arc::new(builder.finish()));
        let lockgate = resources.host.as_ref().expect("initialized host");
        let plugins = Self::classify_plugins(admitted);
        for (id, plugin) in &plugins {
            if !plugin.has_role(&chap_wit::TOOLS) {
                continue;
            }
            for tool in PluginTool::load(
                id,
                Arc::clone(lockgate),
                plugin.handle.clone(),
                config.execution_mode(id),
                plugin_call_deadlines.tool,
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
        plugin_call_deadlines: PluginCallDeadlines,
    ) -> Result<(BTreeMap<String, AdmittedPlugin>, BTreeMap<String, String>), String> {
        let mut plugins = BTreeMap::new();
        let mut plugin_errors = BTreeMap::new();

        for (id, plugin) in config.plugins() {
            match Self::load_plugin(builder, config, consent, id, plugin, plugin_call_deadlines)
                .await?
            {
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
        plugin_call_deadlines: PluginCallDeadlines,
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
        let exported_interfaces = prepared.inspection().exported_interfaces().to_vec();
        let handle = builder
            .admit(
                prepared,
                acceptance,
                runtime_limits(plugin_call_deadlines),
                plugin_admission_context(),
            )
            .await
            .map_err(|error| Self::load_error(id, &path, error))?;
        if let Some(record) = refreshed_record {
            consent.save(record)?;
        }
        Ok(PluginLoad::Admitted(AdmittedPlugin {
            handle,
            exported_interfaces,
            context_channel: plugin.context_channel(),
        }))
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
        let exported_interfaces = prepared.inspection().exported_interfaces();
        Self::validate_supported_role(id, &path, exported_interfaces)?;
        Self::validate_role_config(id, plugin, exported_interfaces)?;
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

    fn validate_role_config(
        id: &str,
        plugin: &ConfiguredPlugin,
        exported_interfaces: &[String],
    ) -> Result<(), String> {
        for role in chap_wit::ROLES {
            if plugin.has_section(role.interface)
                && !exports_interface_named(role.interface, exported_interfaces)
            {
                return Err(format!(
                    "plugin `{id}` configures a `{}` section, but its component does not export the {} interface",
                    role.interface, role.interface,
                ));
            }
        }
        Ok(())
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
        admitted: BTreeMap<String, AdmittedPlugin>,
    ) -> BTreeMap<String, LoadedPlugin> {
        admitted
            .into_iter()
            .map(|(id, admitted)| {
                let roles = chap_wit::ROLES
                    .iter()
                    .filter(|role| {
                        exports_interface_named(role.interface, &admitted.exported_interfaces)
                    })
                    .map(|role| role.interface)
                    .collect();
                let handle = admitted.handle;
                let context_channel = admitted.context_channel;
                (
                    id,
                    LoadedPlugin {
                        handle,
                        roles,
                        context_channel,
                    },
                )
            })
            .collect()
    }
}

fn plugin_admission_context() -> InvocationCtx<()> {
    InvocationCtx::bounded_with_deadline(PLUGIN_FUEL_PER_CALL, PLUGIN_ADMISSION_DEADLINE)
}

fn runtime_limits(deadlines: PluginCallDeadlines) -> RuntimeLimits {
    RuntimeLimits {
        http_request_timeout_ceiling: Some(
            deadlines
                .provider
                .max(deadlines.tool)
                .max(CONTEXT_ASSEMBLY_DEADLINE),
        ),
        ..RuntimeLimits::default()
    }
}

fn supported_roles(interfaces: &[String]) -> Vec<&'static str> {
    chap_wit::ROLES
        .iter()
        .filter(|role| exports_interface_named(role.interface, interfaces))
        .map(|role| role.display_name)
        .collect()
}

fn exports_interface_named(interface: &str, interfaces: &[String]) -> bool {
    let known_interface = <bindings::provider::Role as Role>::INTERFACE;
    let (package, known_name) = known_interface
        .split_once('/')
        .expect("role interfaces must be package-qualified");
    let qualified = match known_name.split_once('@') {
        Some((_, version)) => format!("{package}/{interface}@{version}"),
        None => format!("{package}/{interface}"),
    };
    interfaces.iter().any(|exported| exported == &qualified)
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
            .is_some_and(|plugin| plugin.has_role(&chap_wit::PROVIDER))
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
    Admitted(AdmittedPlugin),
    Refused(String),
}

impl AgentInner {
    async fn run_turn(&self, session: &Session, input: String) -> Result<String, RunError> {
        if !self.sessions.owns(&session.state) {
            return Err(RunError::Other(
                "session does not belong to this runtime".to_owned(),
            ));
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
