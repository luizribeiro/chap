use crate::config::{
    Config, ConfiguredPlugin, LoadError,
    agent::{PluginBudgetSettings, ToolExecutionSettings},
    roles::PluginRoleSettings,
};
use crate::consent::{ConsentError, ConsentStore, PluginConsentReview};
use crate::session::{
    RunError, Session, SessionError, SessionExecutor, SessionFuture, SessionManager, SessionOptions,
};
use crate::tool::ToolRegistry;
use crate::{Tool, ToolDefinition, ToolRegistrationError};
use lockgate::{
    CallBudget, ConsentRecord, ConsentRequired, DriftReport, Host, HostBuilder, PluginConfig,
    PluginHandle, PluginId, Prepared, RequiredEnvironmentVariable, Role, RuntimeLimits,
};
use plugin_tool::PluginTool;
use provider::PluginBackend;
use rustls::RootCertStore;
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
mod telemetry;
mod turn;
#[cfg(feature = "vm")]
#[path = "vm.rs"]
mod vm_host;

const GUEST_HTTP_REQUEST_CEILING: Duration = Duration::from_secs(120);
const MAX_PROVIDER_STEPS_PER_TURN: usize = 64;

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
    AdmissionRefused(Vec<PluginRefusal>),
    #[error("{0}")]
    Consent(#[source] ConsentError),
    #[error("{source}; {cleanup}")]
    OperationAndCleanup {
        #[source]
        source: Box<StartError>,
        cleanup: Box<ConsentError>,
    },
    #[error("failed to clean up refused plugin `{plugin}`: {source}")]
    RefusedPluginCleanup {
        plugin: String,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("{} plugin `{plugin}` failed: {source}", role_label(role))]
    RoleClientUnavailable {
        role: &'static str,
        plugin: String,
        #[source]
        source: lockgate::RoleError,
    },
    #[error("{} plugin `{plugin}` failed: {source}", role_label(role))]
    RoleCallFailed {
        role: &'static str,
        plugin: String,
        #[source]
        source: lockgate::CallError,
    },
    #[error("{} plugin `{plugin}`: {message}", role_label(role))]
    RoleReportedError {
        role: &'static str,
        plugin: String,
        message: String,
    },
    #[error("{0}")]
    ToolRegistration(#[source] ToolRegistrationError),
}

fn role_label(role: &'static str) -> &'static str {
    role.strip_suffix('s').unwrap_or(role)
}

/// Production plugin-call budgets, grouped by exported role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PluginBudgets {
    provider: bindings::provider::Budgets,
    tools: bindings::tools::Budgets,
    context: bindings::context::Budgets,
    admission: CallBudget,
}

impl Default for PluginBudgets {
    fn default() -> Self {
        Self::from(PluginBudgetSettings::default())
    }
}

impl From<PluginBudgetSettings> for PluginBudgets {
    fn from(budgets: PluginBudgetSettings) -> Self {
        Self {
            provider: bindings::provider::Budgets {
                complete: budgets.provider,
            },
            tools: bindings::tools::Budgets {
                definitions: budgets.tools,
                execute: budgets.tools,
            },
            context: bindings::context::Budgets {
                segments: budgets.context,
            },
            admission: budgets.admission,
        }
    }
}

type HostImports = bindings::CapabilityHost;
type InnerHost = Host<()>;
type StartDropResources = (
    Option<ToolRegistry>,
    Option<Arc<InnerHost>>,
    Option<HostBuilder<()>>,
);

async fn host_builder(
    config: &Config,
    budgets: PluginBudgets,
    compiled_cache: Option<PathBuf>,
    tls_roots: Option<RootCertStore>,
) -> Result<HostBuilder<()>, ConsentError> {
    configured_host_builder(config, false, budgets, compiled_cache, tls_roots).await
}

async fn preflight_host_builder(
    config: &Config,
    budgets: PluginBudgets,
    compiled_cache: Option<PathBuf>,
    tls_roots: Option<RootCertStore>,
) -> Result<HostBuilder<()>, ConsentError> {
    configured_host_builder(config, true, budgets, compiled_cache, tls_roots).await
}

async fn configured_host_builder(
    _config: &Config,
    _preflight: bool,
    budgets: PluginBudgets,
    compiled_cache: Option<PathBuf>,
    tls_roots: Option<RootCertStore>,
) -> Result<HostBuilder<()>, ConsentError> {
    #[cfg(feature = "vm")]
    let vm = if _preflight {
        bindings::vm_host::preflight(_config)?
    } else {
        bindings::vm_host::new(_config).await?
    };
    let imports: HostImports = bindings::CapabilityHost {
        #[cfg(feature = "exec")]
        executor: bindings::exec_host::new(_config)?,
        #[cfg(feature = "state")]
        store: bindings::state_host::new(_config)?,
        #[cfg(feature = "vm")]
        vm,
    };
    let builder =
        HostBuilder::new(imports).map_err(|source| ConsentError::HostConstruction { source })?;
    let builder = match tls_roots {
        Some(roots) => builder.tls_roots(roots),
        None => builder,
    };
    let builder = match compiled_cache {
        Some(path) => match fs::create_dir_all(&path) {
            Ok(()) => builder.compiled_cache(path),
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "compiled component cache is unavailable; continuing without it"
                );
                builder
            }
        },
        None => builder,
    };
    let builder = builder
        .budgets::<bindings::provider::Role>(budgets.provider)
        .map_err(|source| ConsentError::InvalidCallBudget { source })?
        .budgets::<bindings::tools::Role>(budgets.tools)
        .map_err(|source| ConsentError::InvalidCallBudget { source })?
        .budgets::<bindings::context::Role>(budgets.context)
        .map_err(|source| ConsentError::InvalidCallBudget { source })?
        .admission_budget(budgets.admission)
        .map_err(|source| ConsentError::InvalidCallBudget { source })?;
    #[cfg(feature = "exec")]
    let builder = builder
        .register::<chap_exec::exec::Contract>()
        .map_err(|source| ConsentError::CapabilityRegistration { source })?;
    #[cfg(feature = "state")]
    let builder = builder
        .register::<chap_state::state::Contract>()
        .map_err(|source| ConsentError::CapabilityRegistration { source })?;
    #[cfg(feature = "vm")]
    let builder = builder
        .register::<chap_vm::vm::Contract>()
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
struct ActivePlugin {
    handle: PluginHandle,
    roles: BTreeSet<&'static str>,
    role_settings: PluginRoleSettings,
}

impl ActivePlugin {
    fn has_role(&self, role: &chap_wit::Role) -> bool {
        self.roles.contains(role.interface)
    }
}

pub struct AgentBuilder {
    config: Config,
    state_dir: Option<PathBuf>,
    tls_roots: Option<RootCertStore>,
    tools: ToolRegistry,
    budgets: PluginBudgets,
}

/// The consent-free coherence result for one configured plugin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginCheck {
    pub instance_id: String,
    pub required_environment_variables: Vec<RequiredEnvironmentVariable>,
}

pub struct Agent {
    inner: Arc<AgentInner>,
}

pub(crate) struct AgentInner {
    lockgate: Arc<InnerHost>,
    plugins: BTreeMap<PluginId, ActivePlugin>,
    sessions: SessionManager,
    tools: ToolRegistry,
    /// Agent-level scheduling settings; see [`crate::config`] for the settings layers.
    tool_execution: ToolExecutionSettings,
}

impl AgentBuilder {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, LoadError> {
        let config = Config::load(path.as_ref())?;
        let budgets = PluginBudgets::from(config.plugin_budgets());
        Ok(Self {
            config,
            state_dir: None,
            tls_roots: None,
            tools: ToolRegistry::new(),
            budgets,
        })
    }

    pub fn state_dir(mut self, state_dir: impl Into<PathBuf>) -> Self {
        self.state_dir = Some(state_dir.into());
        self
    }

    /// Adds private TLS trust anchors for plugin HTTPS requests.
    ///
    /// These anchors extend the public WebPKI roots rather than replacing them.
    pub fn tls_roots(mut self, roots: RootCertStore) -> Self {
        self.tls_roots = Some(roots);
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

    fn compiled_cache_path(&self) -> Option<PathBuf> {
        match self.consent_path() {
            Ok(path) => Some(path.with_file_name("compiled")),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "compiled component cache location is unavailable; continuing without it"
                );
                None
            }
        }
    }

    pub fn plugins(&self) -> impl Iterator<Item = (PluginId, &Path)> {
        self.config
            .plugins()
            .map(|(id, plugin)| (id, plugin.component()))
    }

    pub fn plugin_roles(&self, plugin_id: &PluginId) -> Result<Vec<&'static str>, ConsentError> {
        let plugin =
            self.config
                .plugin(plugin_id)
                .ok_or_else(|| ConsentError::PluginNotConfigured {
                    plugin: plugin_id.as_str().to_owned(),
                })?;
        let bytes = Self::plugin_bytes(&self.config, plugin_id, plugin)?;
        let inspection =
            lockgate::inspect(&bytes).map_err(|source| ConsentError::InspectPlugin {
                plugin: plugin_id.as_str().to_owned(),
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

    /// Checks every configured plugin through smoke instantiation without consent.
    ///
    /// Required environment variables are reported with their current presence,
    /// but an unset variable does not make the coherence check fail.
    pub async fn check_plugins(&self) -> Result<Vec<PluginCheck>, StartError> {
        let builder = preflight_host_builder(
            &self.config,
            self.budgets,
            self.compiled_cache_path(),
            self.tls_roots.clone(),
        )
        .await
        .map_err(StartError::Consent)?;
        let mut resources = StartResources::new(ToolRegistry::new(), builder);
        let result = Self::preflight_plugins(
            resources.builder.as_mut().expect("uninitialized host"),
            &self.config,
        )
        .await;
        match (result, resources.cleanup().await) {
            (Ok(checks), Ok(())) => Ok(checks),
            (Ok(_), Err(cleanup)) => Err(StartError::Consent(cleanup)),
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(StartError::OperationAndCleanup {
                source: Box::new(error),
                cleanup: Box::new(cleanup),
            }),
        }
    }

    async fn preflight_plugins(
        builder: &mut HostBuilder<()>,
        config: &Config,
    ) -> Result<Vec<PluginCheck>, StartError> {
        let mut checks = Vec::new();
        let mut refusals = Vec::new();
        for (plugin_id, plugin) in config.plugins() {
            let path = config.component_path(plugin);
            match Self::preflight_plugin(builder, config, &plugin_id, plugin).await {
                Ok(check) => checks.push(check),
                Err(error) => refusals.push(Self::plugin_refusal(&plugin_id, &path, error)?),
            }
        }
        if refusals.is_empty() {
            Ok(checks)
        } else {
            Err(StartError::AdmissionRefused(refusals))
        }
    }

    async fn preflight_plugin(
        builder: &mut HostBuilder<()>,
        config: &Config,
        plugin_id: &PluginId,
        plugin: &ConfiguredPlugin,
    ) -> Result<PluginCheck, ConsentError> {
        let path = config.component_path(plugin);
        let prepared = Self::prepare_plugin(builder, config, plugin_id, plugin).await?;
        let operation = builder
            .preflight(&prepared, &runtime_limits())
            .await
            .map(|preflight| PluginCheck {
                instance_id: plugin_id.as_str().to_owned(),
                required_environment_variables: preflight.required_environment_variables,
            })
            .map_err(|source| ConsentError::LoadPlugin {
                plugin: plugin_id.as_str().to_owned(),
                path,
                source: Box::new(source),
            });
        let cleanup = Self::cleanup_prepared_plugin(plugin_id, prepared).await;
        match (operation, cleanup) {
            (Ok(check), Ok(())) => Ok(check),
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(ConsentError::OperationAndCleanup {
                source: Box::new(error),
                cleanup: Box::new(cleanup),
            }),
        }
    }

    pub async fn approve_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<ConsentRecord, ConsentError> {
        let consent = self.consent_store()?;
        self.configured_plugin(plugin_id)?;
        let mut resources = StartResources::new(
            ToolRegistry::new(),
            host_builder(
                &self.config,
                self.budgets,
                self.compiled_cache_path(),
                self.tls_roots.clone(),
            )
            .await?,
        );
        let result = self
            .approve_configured_plugin(
                resources.builder.as_mut().expect("uninitialized host"),
                &consent,
                plugin_id,
            )
            .await;
        Self::finish_consent_operation(result, resources).await
    }

    async fn approve_configured_plugin(
        &self,
        builder: &mut HostBuilder<()>,
        consent: &ConsentStore,
        plugin_id: &PluginId,
    ) -> Result<ConsentRecord, ConsentError> {
        let prepared = self.prepare_configured_plugin(builder, plugin_id).await?;
        let record = prepared.approve(now_rfc3339());
        let save_result = consent.save(record.clone());
        let cleanup_result = Self::cleanup_prepared_plugin(plugin_id, prepared).await;

        save_result?;
        cleanup_result?;
        Ok(record)
    }

    pub fn deny_plugin(&self, plugin_id: &PluginId) -> Result<(), ConsentError> {
        if self.config.plugin(plugin_id).is_none() {
            return Err(ConsentError::PluginNotConfigured {
                plugin: plugin_id.as_str().to_owned(),
            });
        }
        self.consent_store()?.remove(plugin_id)
    }

    pub async fn review_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginConsentReview, ConsentError> {
        let mut reviews = self
            .review_plugins(core::slice::from_ref(plugin_id))
            .await?;
        Ok(reviews.pop().expect("one plugin was reviewed"))
    }

    /// Reviews several configured plugins against one shared host builder.
    pub async fn review_plugins(
        &self,
        plugin_ids: &[PluginId],
    ) -> Result<Vec<PluginConsentReview>, ConsentError> {
        let consent = self.consent_store()?;
        for plugin_id in plugin_ids {
            self.configured_plugin(plugin_id)?;
        }
        let mut resources = StartResources::new(
            ToolRegistry::new(),
            host_builder(
                &self.config,
                self.budgets,
                self.compiled_cache_path(),
                self.tls_roots.clone(),
            )
            .await?,
        );
        let result = self
            .review_configured_plugins(
                resources.builder.as_mut().expect("uninitialized host"),
                &consent,
                plugin_ids,
            )
            .await;
        Self::finish_consent_operation(result, resources).await
    }

    async fn review_configured_plugins(
        &self,
        builder: &mut HostBuilder<()>,
        consent: &ConsentStore,
        plugin_ids: &[PluginId],
    ) -> Result<Vec<PluginConsentReview>, ConsentError> {
        let mut reviews = Vec::with_capacity(plugin_ids.len());
        for plugin_id in plugin_ids {
            reviews.push(
                self.review_configured_plugin(builder, consent, plugin_id)
                    .await?,
            );
        }
        Ok(reviews)
    }

    async fn review_configured_plugin(
        &self,
        builder: &mut HostBuilder<()>,
        consent: &ConsentStore,
        plugin_id: &PluginId,
    ) -> Result<PluginConsentReview, ConsentError> {
        let prepared = self.prepare_configured_plugin(builder, plugin_id).await?;
        let manifest = prepared.review();
        let prior = consent.load(plugin_id);
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
        Self::cleanup_prepared_plugin(plugin_id, prepared).await?;
        Ok(review)
    }

    async fn prepare_configured_plugin(
        &self,
        builder: &mut HostBuilder<()>,
        plugin_id: &PluginId,
    ) -> Result<Prepared, ConsentError> {
        let plugin = self.configured_plugin(plugin_id)?;
        Self::prepare_plugin(builder, &self.config, plugin_id, plugin).await
    }

    fn configured_plugin(&self, plugin_id: &PluginId) -> Result<&ConfiguredPlugin, ConsentError> {
        self.config
            .plugin(plugin_id)
            .ok_or_else(|| ConsentError::PluginNotConfigured {
                plugin: plugin_id.as_str().to_owned(),
            })
    }

    async fn cleanup_prepared_plugin(
        plugin_id: &PluginId,
        prepared: Prepared,
    ) -> Result<(), ConsentError> {
        tokio::task::spawn_blocking(move || drop(prepared))
            .await
            .map_err(|source| ConsentError::PreparedPluginCleanup {
                plugin: plugin_id.as_str().to_owned(),
                source,
            })
    }

    async fn finish_consent_operation<T>(
        operation: Result<T, ConsentError>,
        resources: StartResources,
    ) -> Result<T, ConsentError> {
        match (operation, resources.cleanup().await) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(ConsentError::OperationAndCleanup {
                source: Box::new(error),
                cleanup: Box::new(cleanup),
            }),
        }
    }

    /// Starts the agent, admitting every configured plugin.
    ///
    /// Refuses to start unless every configured plugin is admitted.
    pub async fn start(self) -> Result<Agent, StartError> {
        let consent = self.consent_store().map_err(StartError::Consent)?;
        let compiled_cache = self.compiled_cache_path();
        let Self {
            config,
            state_dir: _,
            tls_roots,
            tools,
            budgets,
        } = self;
        let builder = host_builder(&config, budgets, compiled_cache, tls_roots)
            .await
            .map_err(StartError::Consent)?;
        let mut resources = StartResources::new(tools, builder);
        let plugins = match Self::initialize_plugins(&mut resources, &config, &consent).await {
            Ok(plugins) => plugins,
            Err(error) => {
                return match resources.cleanup().await {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(StartError::OperationAndCleanup {
                        source: Box::new(error),
                        cleanup: Box::new(cleanup),
                    }),
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
            }),
        })
    }

    async fn initialize_plugins(
        resources: &mut StartResources,
        config: &Config,
        consent: &ConsentStore,
    ) -> Result<BTreeMap<PluginId, ActivePlugin>, StartError> {
        let plugins = Self::load_plugins(
            resources.builder.as_mut().expect("uninitialized host"),
            config,
            consent,
        )
        .await?;
        let builder = resources.builder.take().expect("uninitialized host");
        resources.host = Some(Arc::new(builder.finish()));
        let lockgate = resources.host.as_ref().expect("initialized host");
        for (plugin_id, plugin) in &plugins {
            if !plugin.has_role(&chap_wit::TOOLS) {
                continue;
            }
            for tool in PluginTool::load(
                plugin_id.as_str(),
                Arc::clone(lockgate),
                plugin.handle.clone(),
                &plugin.role_settings.tools,
            )
            .await?
            {
                resources
                    .tools
                    .as_mut()
                    .expect("initialized tool registry")
                    .register(tool)
                    .map_err(StartError::ToolRegistration)?;
            }
        }
        Ok(plugins)
    }

    async fn load_plugins(
        builder: &mut HostBuilder<()>,
        config: &Config,
        consent: &ConsentStore,
    ) -> Result<BTreeMap<PluginId, ActivePlugin>, StartError> {
        let mut plugins = BTreeMap::new();
        let mut refusals = Vec::new();

        for (plugin_id, plugin) in config.plugins() {
            match Self::load_plugin(builder, config, consent, &plugin_id, plugin).await? {
                PluginLoad::Admitted(admitted) => {
                    plugins.insert(plugin_id, admitted);
                }
                PluginLoad::Refused(error) => {
                    tracing::warn!(plugin = %plugin_id, error = %error, "plugin admission failed");
                    refusals.push(error);
                }
            }
        }

        if refusals.is_empty() {
            Ok(plugins)
        } else {
            Err(StartError::AdmissionRefused(refusals))
        }
    }

    async fn load_plugin(
        builder: &mut HostBuilder<()>,
        config: &Config,
        consent: &ConsentStore,
        plugin_id: &PluginId,
        plugin: &ConfiguredPlugin,
    ) -> Result<PluginLoad, StartError> {
        let path = config.component_path(plugin);
        let prepared = match Self::prepare_plugin(builder, config, plugin_id, plugin).await {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(PluginLoad::Refused(Self::plugin_refusal(
                    plugin_id, &path, error,
                )?));
            }
        };
        let record = consent.load(plugin_id);
        let acceptance = match prepared.accept_reviewed(record.as_ref()) {
            Ok(acceptance) => acceptance,
            Err(required) => {
                let refusal = match builder.preflight(&prepared, &runtime_limits()).await {
                    Ok(_) => Self::consent_refusal(plugin_id, &path, required),
                    Err(source) => Self::plugin_refusal(
                        plugin_id,
                        &path,
                        ConsentError::LoadPlugin {
                            plugin: plugin_id.as_str().to_owned(),
                            path: path.clone(),
                            source: Box::new(source),
                        },
                    )?,
                };
                tokio::task::spawn_blocking(move || drop(prepared))
                    .await
                    .map_err(|source| StartError::RefusedPluginCleanup {
                        plugin: plugin_id.as_str().to_owned(),
                        source,
                    })?;
                return Ok(PluginLoad::Refused(refusal));
            }
        };
        let refreshed_record = record.as_ref().and_then(|prior| {
            let manifest = prepared.review();
            (prior.request_digest != manifest.request_digest
                || prior.exported_interfaces != manifest.exported_interfaces)
                .then(|| ConsentRecord {
                    instance_id: manifest.plugin_id.as_str().to_owned(),
                    request_digest: manifest.request_digest,
                    component_digest: Some(manifest.component_digest),
                    exported_interfaces: manifest.exported_interfaces,
                    grants: manifest.grants,
                    approved_at: prior.approved_at.clone(),
                })
        });
        let roles: BTreeSet<&'static str> = chap_wit::ROLES
            .iter()
            .filter(|role| {
                exports_interface_named(role.interface, prepared.inspection().exported_interfaces())
            })
            .map(|role| role.interface)
            .collect();
        let handle = builder
            .admit(prepared, acceptance, runtime_limits())
            .await
            .map_err(|source| ConsentError::LoadPlugin {
                plugin: plugin_id.as_str().to_owned(),
                path: path.clone(),
                source: Box::new(source),
            });
        let handle = match handle {
            Ok(handle) => handle,
            Err(error) => {
                return Ok(PluginLoad::Refused(Self::plugin_refusal(
                    plugin_id, &path, error,
                )?));
            }
        };
        if let Some(record) = refreshed_record {
            consent.save(record).map_err(StartError::Consent)?;
        }
        Ok(PluginLoad::Admitted(ActivePlugin {
            handle,
            roles,
            role_settings: plugin.role_settings(),
        }))
    }

    async fn prepare_plugin(
        builder: &mut HostBuilder<()>,
        config: &Config,
        plugin_id: &PluginId,
        plugin: &ConfiguredPlugin,
    ) -> Result<Prepared, ConsentError> {
        let path = config.component_path(plugin);
        let bytes = Self::plugin_bytes(config, plugin_id, plugin)?;
        let settings = plugin.settings();
        let prepared = builder
            .prepare(
                plugin_id.clone(),
                &bytes,
                PluginConfig {
                    settings: Some(settings),
                    ..PluginConfig::default()
                },
            )
            .await
            .map_err(|source| ConsentError::LoadPlugin {
                plugin: plugin_id.as_str().to_owned(),
                path: path.clone(),
                source: Box::new(source),
            })?;
        let exported_interfaces = prepared.inspection().exported_interfaces();
        Self::validate_supported_role(plugin_id, &path, exported_interfaces)?;
        Self::validate_role_config(plugin_id, plugin, exported_interfaces)?;
        Ok(prepared)
    }

    fn consent_refusal(
        plugin_id: &PluginId,
        path: &Path,
        required: ConsentRequired,
    ) -> PluginRefusal {
        let reason = match required {
            ConsentRequired::FirstRun { .. } => PluginRefusalReason::ApprovalRequired,
            ConsentRequired::Drift { drift, .. } => {
                PluginRefusalReason::RenewedApprovalRequired { drift }
            }
        };
        PluginRefusal {
            instance_id: plugin_id.as_str().to_owned(),
            source_path: path.to_path_buf(),
            reason,
        }
    }

    fn plugin_refusal(
        plugin_id: &PluginId,
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
            error => return Err(StartError::Consent(error)),
        };
        Ok(PluginRefusal {
            instance_id: plugin_id.as_str().to_owned(),
            source_path: path.to_path_buf(),
            reason,
        })
    }

    fn validate_supported_role(
        plugin_id: &PluginId,
        path: &Path,
        exported_interfaces: &[String],
    ) -> Result<(), ConsentError> {
        if !supported_roles(exported_interfaces).is_empty() {
            return Ok(());
        }
        Err(ConsentError::UnsupportedRole {
            plugin: plugin_id.as_str().to_owned(),
            path: path.to_path_buf(),
            role: role_package(<bindings::provider::Role as Role>::INTERFACE),
            exported_interfaces: exported_interfaces.to_vec(),
        })
    }

    fn validate_role_config(
        plugin_id: &PluginId,
        plugin: &ConfiguredPlugin,
        exported_interfaces: &[String],
    ) -> Result<(), ConsentError> {
        for role in chap_wit::ROLES {
            if plugin.has_section(role.interface)
                && !exports_interface_named(role.interface, exported_interfaces)
            {
                return Err(ConsentError::RoleConfigInvalid {
                    plugin: plugin_id.as_str().to_owned(),
                    role: role.interface.to_owned(),
                });
            }
        }
        Ok(())
    }

    fn plugin_bytes(
        config: &Config,
        plugin_id: &PluginId,
        plugin: &ConfiguredPlugin,
    ) -> Result<Vec<u8>, ConsentError> {
        let path = config.component_path(plugin);
        fs::read(&path).map_err(|source| ConsentError::ReadPlugin {
            plugin: plugin_id.as_str().to_owned(),
            path,
            source,
        })
    }
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
            .get(&PluginId::from(options.provider.as_str()))
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
    Admitted(ActivePlugin),
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
