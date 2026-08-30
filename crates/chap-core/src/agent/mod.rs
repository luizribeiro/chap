use crate::config::{
    Config, ConfiguredPlugin, LoadError, agent::ToolExecutionSettings, roles::PluginRoleSettings,
};
use crate::consent::{ConsentError, ConsentStore, PluginConsentReview};
use crate::session::{
    RunError, Session, SessionError, SessionExecutor, SessionFuture, SessionManager, SessionOptions,
};
use crate::tool::ToolRegistry;
use crate::{Tool, ToolDefinition, ToolRegistrationError};
use lockgate::{
    ConsentRecord, ConsentRequired, DriftReport, Host, HostBuilder, InvocationCtx, PluginConfig,
    PluginHandle, Prepared, Role, RuntimeLimits,
};
use plugin_tool::PluginTool;
use provider::PluginBackend;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use turn::run_agent_loop;

mod bindings;
mod context;
mod plugin_tool;
mod provider;
mod turn;

const PLUGIN_FUEL_PER_CALL: u64 = 25_000_000;
const PLUGIN_ADMISSION_DEADLINE: Duration = Duration::from_secs(30);
const GUEST_HTTP_REQUEST_CEILING: Duration = Duration::from_secs(120);
const MAX_PROVIDER_STEPS_PER_TURN: usize = 64;

macro_rules! define_plugin_calls {
    ($(
        $(#[$meta:meta])*
        $variant:ident => ($interface:literal, $function:literal),
    )+) => {
        /// An exported WIT function callable on a plugin.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum PluginCall {
            $(
                $(#[$meta])*
                $variant,
            )+
        }

        impl PluginCall {
            #[cfg(test)]
            const ALL: &'static [Self] = &[$(Self::$variant),+];

            #[cfg(test)]
            const fn wit_name(self) -> (&'static str, &'static str) {
                match self {
                    $(Self::$variant => ($interface, $function),)+
                }
            }
        }
    };
}

define_plugin_calls! {
    /// `provider.complete`.
    ProviderComplete => ("provider", "complete"),
    /// `tools.definitions`.
    ToolDefinitions => ("tools", "definitions"),
    /// `tools.execute`.
    ToolExecute => ("tools", "execute"),
    /// `context.segments`.
    ContextSegments => ("context", "segments"),
}

/// Fuel and wall-clock bounds for one exported plugin call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallBudget {
    /// Maximum guest instructions consumed by the call.
    pub fuel: u64,
    /// Maximum wall-clock duration of the call.
    pub deadline: Duration,
}

impl CallBudget {
    /// Creates the bounded invocation context used to make the call.
    pub fn invocation_context(self) -> InvocationCtx<()> {
        InvocationCtx::bounded(self.fuel, self.deadline)
    }
}

#[derive(Debug, Error)]
#[error(
    "plugin `{instance_id}` from `{}` was refused admission: {reason}",
    source_path.display()
)]
pub struct PluginRefusal {
    pub instance_id: String,
    pub source_path: PathBuf,
    #[source]
    pub reason: PluginRefusalReason,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PluginRefusalReason {
    #[error("requires approval before admission")]
    ApprovalRequired,
    #[error("requires renewed approval before admission")]
    RenewedApprovalRequired { drift: DriftReport },
    #[error("does not implement a supported role from `{role}`")]
    UnsupportedRole {
        role: String,
        exported_interfaces: Vec<String>,
    },
    #[error(
        "configures a `{role}` section, but its component does not export the {role} interface"
    )]
    RoleConfigInvalid { role: String },
    #[error("could not be loaded: {source}")]
    ComponentLoad {
        #[source]
        source: Box<ConsentError>,
    },
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StartError {
    #[error("{} plugin(s) were refused admission", .0.len())]
    Refused(Vec<PluginRefusal>),
    /// Deliberate temporary catch-all for start failures not yet typed in issue #9.
    #[error("{0}")]
    Internal(String),
}

/// Production plugin-call budgets, keyed by exported WIT function.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CallBudgets {
    provider_complete: CallBudget,
    tool_definitions: CallBudget,
    tool_execute: CallBudget,
    context_segments: CallBudget,
}

impl CallBudgets {
    fn resolve(self, call: PluginCall) -> CallBudget {
        match call {
            PluginCall::ProviderComplete => self.provider_complete,
            PluginCall::ToolDefinitions => self.tool_definitions,
            PluginCall::ToolExecute => self.tool_execute,
            PluginCall::ContextSegments => self.context_segments,
        }
    }

    fn set(&mut self, call: PluginCall, budget: CallBudget) {
        match call {
            PluginCall::ProviderComplete => self.provider_complete = budget,
            PluginCall::ToolDefinitions => self.tool_definitions = budget,
            PluginCall::ToolExecute => self.tool_execute = budget,
            PluginCall::ContextSegments => self.context_segments = budget,
        }
    }
}

impl Default for CallBudgets {
    fn default() -> Self {
        Self {
            provider_complete: CallBudget {
                fuel: PLUGIN_FUEL_PER_CALL,
                deadline: Duration::from_secs(120),
            },
            tool_definitions: CallBudget {
                fuel: PLUGIN_FUEL_PER_CALL,
                deadline: Duration::from_secs(30),
            },
            tool_execute: CallBudget {
                fuel: PLUGIN_FUEL_PER_CALL,
                deadline: Duration::from_secs(30),
            },
            context_segments: CallBudget {
                fuel: PLUGIN_FUEL_PER_CALL,
                deadline: Duration::from_secs(10),
            },
        }
    }
}

#[cfg(not(feature = "exec"))]
type HostImports = ();
#[cfg(feature = "exec")]
type HostImports = bindings::ExecImports;
type InnerHost = Host<()>;
type StartDropResources = (
    Option<ToolRegistry>,
    Option<Arc<InnerHost>>,
    Option<HostBuilder<()>>,
);

fn host_builder(_config: &Config) -> Result<HostBuilder<()>, ConsentError> {
    #[cfg(not(feature = "exec"))]
    let imports: HostImports = ();
    #[cfg(feature = "exec")]
    let imports: HostImports = {
        let project_root =
            std::env::current_dir().map_err(|source| ConsentError::CurrentDirectory { source })?;
        bindings::ExecImports::new(
            _config
                .exec_config()
                .map_err(ConsentError::HostConfiguration)?,
            &project_root,
        )
    };
    let builder =
        HostBuilder::new(imports).map_err(|source| ConsentError::HostConstruction { source })?;
    #[cfg(feature = "exec")]
    let builder = builder
        .register::<chap_exec::exec::Contract>()
        .map_err(|source| ConsentError::CapabilityRegistration { source })?;
    Ok(builder)
}

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

    async fn cleanup(mut self) -> Result<(), ConsentError> {
        let resources = self.take_drop_resources();
        tokio::task::spawn_blocking(move || drop(resources))
            .await
            .map_err(|source| ConsentError::HostCleanup { source })
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
    role_settings: PluginRoleSettings,
}

struct AdmittedPlugin {
    handle: PluginHandle,
    exported_interfaces: Vec<String>,
    role_settings: PluginRoleSettings,
}

impl LoadedPlugin {
    fn has_role(&self, role: &chap_wit::Role) -> bool {
        self.roles.contains(role.interface)
    }
}

pub struct AgentBuilder {
    config: Config,
    state_dir: Option<PathBuf>,
    tools: ToolRegistry,
    call_budgets: CallBudgets,
}

pub struct Agent {
    inner: Arc<AgentInner>,
}

pub(crate) struct AgentInner {
    lockgate: Arc<InnerHost>,
    plugins: BTreeMap<String, LoadedPlugin>,
    sessions: SessionManager,
    tools: ToolRegistry,
    tool_execution: ToolExecutionSettings,
    call_budgets: CallBudgets,
}

impl AgentBuilder {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, LoadError> {
        let config = Config::load(path.as_ref())?;
        Ok(Self {
            config,
            state_dir: None,
            tools: ToolRegistry::new(),
            call_budgets: CallBudgets::default(),
        })
    }

    pub fn state_dir(mut self, state_dir: impl Into<PathBuf>) -> Self {
        self.state_dir = Some(state_dir.into());
        self
    }

    pub fn consent_path(&self) -> Result<PathBuf, LoadError> {
        match &self.state_dir {
            Some(state_dir) => Ok(state_dir.join("consent.json")),
            None => self.config.consent_path(),
        }
    }

    fn consent_store(&self) -> Result<ConsentStore, ConsentError> {
        Ok(ConsentStore::new(
            self.consent_path().map_err(ConsentError::StateLocation)?,
            self.config.source_path(),
        ))
    }

    /// Overrides the execution budget for one exported plugin call on every plugin.
    ///
    /// Calls without an override retain their production fuel and deadline defaults.
    pub fn call_budget(mut self, call: PluginCall, budget: CallBudget) -> Self {
        self.call_budgets.set(call, budget);
        self
    }

    pub fn plugins(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.config
            .plugins()
            .map(|(id, plugin)| (id, plugin.component()))
    }

    pub fn plugin_roles(&self, id: &str) -> Result<Vec<&'static str>, ConsentError> {
        let plugin = self
            .config
            .plugin(id)
            .ok_or_else(|| ConsentError::PluginNotConfigured {
                plugin: id.to_owned(),
            })?;
        let bytes = Self::plugin_bytes(&self.config, id, plugin)?;
        let inspection =
            lockgate::inspect(&bytes).map_err(|source| ConsentError::InspectPlugin {
                plugin: id.to_owned(),
                source: Box::new(source),
            })?;
        Ok(supported_roles(inspection.exported_interfaces()))
    }

    pub fn tool<T>(mut self, tool: T) -> Result<Self, ToolRegistrationError>
    where
        T: Tool + 'static,
    {
        self.tools.register(tool)?;
        Ok(self)
    }

    pub async fn approve_plugin(&self, id: &str) -> Result<ConsentRecord, ConsentError> {
        let consent = self.consent_store()?;
        let (prepared, resources) = self.prepare_configured_plugin(id).await?;
        let record = prepared.approve(now_rfc3339());
        let save_result = consent.save(record.clone());
        let cleanup_result = Self::cleanup_prepared_plugin(id, prepared, resources).await;

        save_result?;
        cleanup_result?;
        Ok(record)
    }

    pub fn deny_plugin(&self, id: &str) -> Result<(), ConsentError> {
        if self.config.plugin(id).is_none() {
            return Err(ConsentError::PluginNotConfigured {
                plugin: id.to_owned(),
            });
        }
        self.consent_store()?.remove(id)
    }

    pub async fn review_plugin(&self, id: &str) -> Result<PluginConsentReview, ConsentError> {
        let consent = self.consent_store()?;
        let (prepared, resources) = self.prepare_configured_plugin(id).await?;
        let manifest = prepared.review();
        let prior = consent.load(id);
        let drift = prior
            .as_ref()
            .filter(|prior| {
                prior.request_digest != manifest.request_digest
                    || prior.exported_interfaces != manifest.exported_interfaces
            })
            .map(|prior| lockgate::consent_drift(prior, &manifest));
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
    ) -> Result<(Prepared, StartResources), ConsentError> {
        let plugin = self
            .config
            .plugin(id)
            .ok_or_else(|| ConsentError::PluginNotConfigured {
                plugin: id.to_owned(),
            })?;
        let builder = host_builder(&self.config)?;
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
                    Err(cleanup) => Err(ConsentError::OperationAndCleanup {
                        source: Box::new(error),
                        cleanup: Box::new(cleanup),
                    }),
                };
            }
        };
        Ok((prepared, resources))
    }

    async fn cleanup_prepared_plugin(
        id: &str,
        prepared: Prepared,
        resources: StartResources,
    ) -> Result<(), ConsentError> {
        let prepared_cleanup = tokio::task::spawn_blocking(move || drop(prepared))
            .await
            .map_err(|source| ConsentError::PreparedPluginCleanup {
                plugin: id.to_owned(),
                source,
            });
        let resource_cleanup = resources.cleanup().await;

        prepared_cleanup?;
        resource_cleanup?;
        Ok(())
    }

    /// Starts the agent, admitting every configured plugin.
    ///
    /// Refuses to start unless every configured plugin is admitted.
    pub async fn start(self) -> Result<Agent, StartError> {
        let consent = self
            .consent_store()
            .map_err(|error| StartError::Internal(error.to_string()))?;
        let Self {
            config,
            state_dir: _,
            tools,
            call_budgets,
        } = self;
        let builder =
            host_builder(&config).map_err(|error| StartError::Internal(error.to_string()))?;
        let mut resources = StartResources::new(tools, builder);
        let plugins =
            match Self::initialize_plugins(&mut resources, &config, &consent, call_budgets).await {
                Ok(plugins) => plugins,
                Err(error) => {
                    return match resources.cleanup().await {
                        Ok(()) => Err(error),
                        Err(cleanup_error) => {
                            Err(StartError::Internal(format!("{error}; {cleanup_error}")))
                        }
                    };
                }
            };
        let lockgate = resources.host.take().expect("initialized Lockgate host");
        let tools = resources.tools.take().expect("initialized tool registry");
        let tool_execution = config.tool_execution();
        Ok(Agent {
            inner: Arc::new(AgentInner {
                lockgate,
                plugins,
                sessions: SessionManager::new(),
                tools,
                tool_execution,
                call_budgets,
            }),
        })
    }

    async fn initialize_plugins(
        resources: &mut StartResources,
        config: &Config,
        consent: &ConsentStore,
        call_budgets: CallBudgets,
    ) -> Result<BTreeMap<String, LoadedPlugin>, StartError> {
        let admitted = Self::load_plugins(
            resources.builder.as_mut().expect("uninitialized host"),
            config,
            consent,
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
                plugin.role_settings.tools(),
                call_budgets,
            )
            .await
            .map_err(StartError::Internal)?
            {
                resources
                    .tools
                    .as_mut()
                    .expect("initialized tool registry")
                    .register(tool)
                    .map_err(|error| StartError::Internal(error.to_string()))?;
            }
        }
        Ok(plugins)
    }

    async fn load_plugins(
        builder: &mut HostBuilder<()>,
        config: &Config,
        consent: &ConsentStore,
    ) -> Result<BTreeMap<String, AdmittedPlugin>, StartError> {
        let mut plugins = BTreeMap::new();
        let mut refusals = Vec::new();

        for (id, plugin) in config.plugins() {
            match Self::load_plugin(builder, config, consent, id, plugin).await? {
                PluginLoad::Admitted(admitted) => {
                    plugins.insert(id.to_owned(), admitted);
                }
                PluginLoad::Refused(error) => refusals.push(error),
            }
        }

        if refusals.is_empty() {
            Ok(plugins)
        } else {
            Err(StartError::Refused(refusals))
        }
    }

    async fn load_plugin(
        builder: &mut HostBuilder<()>,
        config: &Config,
        consent: &ConsentStore,
        id: &str,
        plugin: &ConfiguredPlugin,
    ) -> Result<PluginLoad, StartError> {
        let path = config.component_path(plugin);
        let prepared = match Self::prepare_plugin(builder, config, id, plugin).await {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(PluginLoad::Refused(Self::plugin_refusal(id, &path, error)?));
            }
        };
        let record = consent.load(id);
        let acceptance = match prepared.accept_reviewed(record.as_ref()) {
            Ok(acceptance) => acceptance,
            Err(required) => {
                let refusal = Self::consent_refusal(id, &path, required);
                tokio::task::spawn_blocking(move || drop(prepared))
                    .await
                    .map_err(|error| {
                        StartError::Internal(format!(
                            "failed to clean up refused plugin `{id}`: {error}"
                        ))
                    })?;
                return Ok(PluginLoad::Refused(refusal));
            }
        };
        let refreshed_record = record.as_ref().and_then(|prior| {
            let manifest = prepared.review();
            (prior.request_digest != manifest.request_digest
                || prior.exported_interfaces != manifest.exported_interfaces)
                .then(|| ConsentRecord {
                    instance_id: manifest.instance_id,
                    request_digest: manifest.request_digest,
                    component_digest: Some(manifest.component_digest),
                    exported_interfaces: manifest.exported_interfaces,
                    grants: manifest.grants,
                    approved_at: prior.approved_at.clone(),
                })
        });
        let exported_interfaces = prepared.inspection().exported_interfaces().to_vec();
        let handle = builder
            .admit(
                prepared,
                acceptance,
                runtime_limits(),
                plugin_admission_context(),
            )
            .await
            .map_err(|source| ConsentError::LoadPlugin {
                plugin: id.to_owned(),
                path: path.clone(),
                source: Box::new(source),
            });
        let handle = match handle {
            Ok(handle) => handle,
            Err(error) => {
                return Ok(PluginLoad::Refused(Self::plugin_refusal(id, &path, error)?));
            }
        };
        if let Some(record) = refreshed_record {
            consent
                .save(record)
                .map_err(|error| StartError::Internal(error.to_string()))?;
        }
        Ok(PluginLoad::Admitted(AdmittedPlugin {
            handle,
            exported_interfaces,
            role_settings: plugin.role_settings(),
        }))
    }

    async fn prepare_plugin(
        builder: &mut HostBuilder<()>,
        config: &Config,
        id: &str,
        plugin: &ConfiguredPlugin,
    ) -> Result<Prepared, ConsentError> {
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
            .map_err(|source| ConsentError::LoadPlugin {
                plugin: id.to_owned(),
                path: path.clone(),
                source: Box::new(source),
            })?;
        let exported_interfaces = prepared.inspection().exported_interfaces();
        Self::validate_supported_role(id, &path, exported_interfaces)?;
        Self::validate_role_config(id, plugin, exported_interfaces)?;
        Ok(prepared)
    }

    fn consent_refusal(id: &str, path: &Path, required: ConsentRequired) -> PluginRefusal {
        let reason = match required {
            ConsentRequired::FirstRun { .. } => PluginRefusalReason::ApprovalRequired,
            ConsentRequired::Drift { drift, .. } => {
                PluginRefusalReason::RenewedApprovalRequired { drift }
            }
        };
        PluginRefusal {
            instance_id: id.to_owned(),
            source_path: path.to_path_buf(),
            reason,
        }
    }

    fn plugin_refusal(
        id: &str,
        path: &Path,
        error: ConsentError,
    ) -> Result<PluginRefusal, StartError> {
        let reason = match error {
            source @ (ConsentError::ReadPlugin { .. } | ConsentError::LoadPlugin { .. }) => {
                PluginRefusalReason::ComponentLoad {
                    source: Box::new(source),
                }
            }
            ConsentError::UnsupportedRole {
                role,
                exported_interfaces,
                ..
            } => PluginRefusalReason::UnsupportedRole {
                role,
                exported_interfaces,
            },
            ConsentError::RoleConfigInvalid { role, .. } => {
                PluginRefusalReason::RoleConfigInvalid { role }
            }
            error => return Err(StartError::Internal(error.to_string())),
        };
        Ok(PluginRefusal {
            instance_id: id.to_owned(),
            source_path: path.to_path_buf(),
            reason,
        })
    }

    fn validate_supported_role(
        id: &str,
        path: &Path,
        exported_interfaces: &[String],
    ) -> Result<(), ConsentError> {
        if !supported_roles(exported_interfaces).is_empty() {
            return Ok(());
        }
        Err(ConsentError::UnsupportedRole {
            plugin: id.to_owned(),
            path: path.to_path_buf(),
            role: role_package(<bindings::provider::Role as Role>::INTERFACE),
            exported_interfaces: exported_interfaces.to_vec(),
        })
    }

    fn validate_role_config(
        id: &str,
        plugin: &ConfiguredPlugin,
        exported_interfaces: &[String],
    ) -> Result<(), ConsentError> {
        for role in chap_wit::ROLES {
            if plugin.has_section(role.interface)
                && !exports_interface_named(role.interface, exported_interfaces)
            {
                return Err(ConsentError::RoleConfigInvalid {
                    plugin: id.to_owned(),
                    role: role.interface.to_owned(),
                });
            }
        }
        Ok(())
    }

    fn plugin_bytes(
        config: &Config,
        id: &str,
        plugin: &ConfiguredPlugin,
    ) -> Result<Vec<u8>, ConsentError> {
        let path = config.component_path(plugin);
        fs::read(&path).map_err(|source| ConsentError::ReadPlugin {
            plugin: id.to_owned(),
            path,
            source,
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
                let role_settings = admitted.role_settings;
                (
                    id,
                    LoadedPlugin {
                        handle,
                        roles,
                        role_settings,
                    },
                )
            })
            .collect()
    }
}

fn plugin_admission_context() -> InvocationCtx<()> {
    InvocationCtx::bounded(PLUGIN_FUEL_PER_CALL, PLUGIN_ADMISSION_DEADLINE)
}

fn runtime_limits() -> RuntimeLimits {
    RuntimeLimits {
        http_request_timeout_ceiling: Some(GUEST_HTTP_REQUEST_CEILING),
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

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("the current UTC time must be representable as RFC3339")
}

impl Agent {
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.inner.tools.definitions()
    }

    pub async fn session(&self, options: SessionOptions) -> Result<Session, SessionError> {
        if !self
            .inner
            .plugins
            .get(&options.provider)
            .is_some_and(|plugin| plugin.has_role(&chap_wit::PROVIDER))
        {
            return Err(SessionError::ProviderNotConfigured {
                provider: options.provider,
            });
        }
        let assembled_context = self
            .inner
            .assemble_context()
            .await
            .map_err(SessionError::Context)?;
        let state = self.inner.sessions.create(options, assembled_context)?;
        Ok(Session::new(state, self.inner.clone()))
    }
}

#[allow(clippy::large_enum_variant)]
enum PluginLoad {
    Admitted(AdmittedPlugin),
    Refused(PluginRefusal),
}

impl AgentInner {
    async fn run_turn(&self, session: &Session, input: String) -> Result<String, RunError> {
        if !self.sessions.owns(&session.state) {
            return Err(RunError::ForeignSession);
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
