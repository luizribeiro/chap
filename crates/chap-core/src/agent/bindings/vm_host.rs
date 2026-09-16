use super::{CapabilityHost, vm};
use crate::{
    agent::{
        ResolvedWorkspace,
        vm_host::{ManagedVm, WorkspaceResource, translate_host_error, translate_requested_config},
    },
    config::Config,
    consent::ConsentError,
};
use chap_vm::host::{
    Backend, ExecOutcome as VmExecOutcome, RequestedVmConfig, VmBackend, VmCommand, VmConfig,
    VmIdentity, VmPrincipal, VmRef, VmSettings,
};
use lockgate::{HostCtx, PermissionDenied, PluginId, PluginSubject, ResolveScopedResource};
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const STARTUP_REAP_OPERATION: &str = "reap stale VMs at startup";
const SHUTDOWN_REAP_OPERATION: &str = "reap session VMs at shutdown";
const WORKSPACE_HANDLE: &str = "@workspace";
const WORKSPACE_LOGICAL_NAME: &str = "workspace";

fn reject_reserved_name(name: &str) -> Result<(), vm::VmError> {
    if name.starts_with('@') {
        Err(vm::VmError::Denied(
            "VM names starting with `@` use a reserved prefix".into(),
        ))
    } else {
        Ok(())
    }
}

fn warn_lifecycle_failure(operation: &'static str, error: impl fmt::Display) {
    tracing::warn!(operation, error = %error, "VM lifecycle operation failed");
}

pub(crate) struct VmHost<B: VmBackend = Backend> {
    pub(crate) backend: Arc<B>,
    pub(crate) settings: VmSettings,
    pub(crate) installation_id: String,
    pub(crate) session_epoch: u64,
    pub(crate) vm_counts: Arc<Mutex<HashMap<PluginId, u32>>>,
    pub(crate) workspace: Option<ResolvedWorkspace>,
    workspace_vm: Arc<tokio::sync::OnceCell<VmRef>>,
    cleanup: Arc<VmCleanup<B>>,
}

impl<B: VmBackend> Clone for VmHost<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            settings: self.settings.clone(),
            installation_id: self.installation_id.clone(),
            session_epoch: self.session_epoch,
            vm_counts: Arc::clone(&self.vm_counts),
            workspace: self.workspace.clone(),
            workspace_vm: Arc::clone(&self.workspace_vm),
            cleanup: Arc::clone(&self.cleanup),
        }
    }
}

pub(in crate::agent) async fn new(
    config: &Config,
    workspace: Option<ResolvedWorkspace>,
) -> Result<VmHost, ConsentError> {
    let project_root = std::env::current_dir()
        .map_err(|source| ConsentError::CurrentDirectoryUnavailable { source })?;
    let settings = config
        .vm_settings()
        .map_err(ConsentError::HostConfiguration)?;
    VmHost::with_backend(
        Arc::new(Backend::new(&settings)),
        settings,
        project_root.to_string_lossy().into_owned(),
        session_epoch(),
        workspace,
    )
    .await
    .map_err(|source| ConsentError::VmLifecycle {
        operation: "reap stale VMs",
        source,
    })
}

pub(in crate::agent) fn preflight(
    config: &Config,
    workspace: Option<ResolvedWorkspace>,
) -> Result<VmHost, ConsentError> {
    let project_root = std::env::current_dir()
        .map_err(|source| ConsentError::CurrentDirectoryUnavailable { source })?;
    let settings = config
        .vm_settings()
        .map_err(ConsentError::HostConfiguration)?;
    Ok(VmHost::without_lifecycle(
        Arc::new(Backend::new(&settings)),
        settings,
        project_root.to_string_lossy().into_owned(),
        session_epoch(),
        workspace,
    ))
}

impl<B: VmBackend> VmHost<B> {
    async fn with_backend(
        backend: Arc<B>,
        settings: VmSettings,
        installation_id: String,
        session_epoch: u64,
        workspace: Option<ResolvedWorkspace>,
    ) -> Result<Self, chap_vm::host::VmError> {
        let destroy_timeout = Duration::from_millis(settings.calls.destroy_timeout_ms);
        if let Err(error) = match tokio::time::timeout(
            destroy_timeout,
            backend.reap(&installation_id, session_epoch),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(chap_vm::host::VmError::TimedOut),
        } {
            warn_lifecycle_failure(STARTUP_REAP_OPERATION, error);
        }
        Ok(Self {
            cleanup: Arc::new(VmCleanup {
                backend: Arc::clone(&backend),
                installation_id: installation_id.clone(),
                session_epoch,
                timeout: destroy_timeout,
                shutdown_on_drop: true,
            }),
            backend,
            settings,
            installation_id,
            session_epoch,
            vm_counts: Arc::default(),
            workspace,
            workspace_vm: Arc::default(),
        })
    }

    fn without_lifecycle(
        backend: Arc<B>,
        settings: VmSettings,
        installation_id: String,
        session_epoch: u64,
        workspace: Option<ResolvedWorkspace>,
    ) -> Self {
        Self {
            cleanup: Arc::new(VmCleanup {
                backend: Arc::clone(&backend),
                installation_id: installation_id.clone(),
                session_epoch,
                timeout: Duration::from_millis(settings.calls.destroy_timeout_ms),
                shutdown_on_drop: false,
            }),
            backend,
            settings,
            installation_id,
            session_epoch,
            vm_counts: Arc::default(),
            workspace,
            workspace_vm: Arc::default(),
        }
    }

    pub(crate) fn identity(&self, subject: &PluginSubject<'_>, logical_name: &str) -> VmIdentity {
        VmIdentity {
            installation_id: self.installation_id.clone(),
            session_epoch: self.session_epoch,
            principal: VmPrincipal::Plugin(subject.plugin_id().clone()),
            logical_name: logical_name.to_owned(),
        }
    }

    fn workspace_identity(&self) -> VmIdentity {
        VmIdentity {
            installation_id: self.installation_id.clone(),
            session_epoch: self.session_epoch,
            principal: VmPrincipal::Host,
            logical_name: WORKSPACE_LOGICAL_NAME.into(),
        }
    }

    fn workspace_config(&self) -> Result<&ResolvedWorkspace, vm::VmError> {
        self.workspace
            .as_ref()
            .ok_or_else(|| vm::VmError::Failed("no workspace is configured for this agent".into()))
    }

    fn validate_get_name(&self, name: &str) -> Result<(), vm::VmError> {
        if name != WORKSPACE_HANDLE {
            return Ok(());
        }
        self.workspace_config()?;
        Err(vm::VmError::Denied(
            "the workspace is owned by the host".into(),
        ))
    }

    fn workspace_record(&self) -> Result<vm::WorkspaceInfo, vm::VmError> {
        let workspace = self.workspace_config()?;
        Ok(vm::WorkspaceInfo {
            vm: WORKSPACE_HANDLE.into(),
            image: workspace.info.image.clone(),
            mounts: workspace
                .requested
                .mounts
                .iter()
                .map(|mount| vm::WorkspaceMount {
                    guest: mount.guest.clone(),
                    readonly: mount.readonly,
                })
                .collect(),
            egress: workspace.info.egress.clone(),
            secrets: workspace
                .info
                .secrets
                .iter()
                .map(|secret| vm::WorkspaceSecret {
                    env: secret.env.clone(),
                    hosts: secret.hosts.clone(),
                })
                .collect(),
        })
    }

    fn resolve_workspace(&self) -> Result<ManagedVm, vm::VmError> {
        self.workspace_config()?;
        Ok(ManagedVm::Workspace)
    }

    async fn workspace_ref(&self) -> Result<VmRef, vm::VmError> {
        let workspace = self.workspace_config()?;
        Ok(self
            .workspace_vm
            .get_or_try_init(|| async {
                let id = self.workspace_identity();
                let config = self.backend_config(workspace.requested.clone())?;
                self.backend_call(
                    self.settings.calls.create_timeout_ms,
                    self.backend.get_or_create(&id, &config),
                )
                .await
            })
            .await?
            .clone())
    }

    async fn live_ref(&self, vm: &ManagedVm) -> Result<VmRef, vm::VmError> {
        match vm {
            ManagedVm::CreatedByCaller { vm_ref } => Ok(vm_ref.clone()),
            ManagedVm::Workspace => self.workspace_ref().await,
        }
    }

    async fn resolve_owned(
        &self,
        subject: &PluginSubject<'_>,
        name: &str,
    ) -> Result<ManagedVm, vm::VmError> {
        let id = self.identity(subject, name);
        self.resolve_owned_identity(&id, subject.plugin_id()).await
    }

    async fn resolve_owned_identity(
        &self,
        id: &VmIdentity,
        plugin_id: &PluginId,
    ) -> Result<ManagedVm, vm::VmError> {
        let Some(vm_ref) = self.backend.get(id).await? else {
            // Physical identity includes the caller principal, so absence reveals no other VM.
            return Err(vm::VmError::NoSuchVm);
        };
        let principal = VmPrincipal::Plugin(plugin_id.clone());
        if self.backend.owner_of(&vm_ref).await?.as_ref() != Some(&principal) {
            return Err(vm::VmError::NoSuchVm);
        }
        Ok(ManagedVm::CreatedByCaller { vm_ref })
    }

    fn backend_config(&self, requested: RequestedVmConfig) -> Result<VmConfig, vm::VmError> {
        let config_hash = requested.canonical_hash();
        let image = requested
            .image
            .resolve(&self.settings)
            .map_err(translate_host_error)?;
        Ok(VmConfig {
            image,
            mounts: requested.mounts,
            egress: requested.egress,
            env: requested.env,
            secrets: requested.secrets,
            cpus: self.settings.instance.cpus,
            memory_mb: self.settings.instance.memory_mb,
            max_lifetime_ms: self.settings.instance.max_lifetime_ms,
            idle_timeout_ms: self.settings.instance.idle_timeout_ms,
            config_hash,
        })
    }

    fn enforce_vm_limit(&self, count: u32) -> Result<(), vm::VmError> {
        if count >= self.settings.limits.max_vms_per_plugin {
            Err(vm::VmError::Denied(format!(
                "maximum of {} VMs per plugin reached",
                self.settings.limits.max_vms_per_plugin
            )))
        } else {
            Ok(())
        }
    }

    fn counts(&self) -> MutexGuard<'_, HashMap<PluginId, u32>> {
        self.vm_counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn reserve_vm(&self, plugin_id: &PluginId) -> Result<VmReservation, vm::VmError> {
        let mut counts = self.counts();
        let count = counts.get(plugin_id).copied().unwrap_or_default();
        self.enforce_vm_limit(count)?;
        counts.insert(plugin_id.clone(), count + 1);
        drop(counts);
        Ok(VmReservation {
            counts: Arc::clone(&self.vm_counts),
            plugin_id: plugin_id.clone(),
            active: true,
        })
    }

    async fn backend_call<T, F>(&self, timeout_ms: u64, call: F) -> Result<T, vm::VmError>
    where
        F: Future<Output = Result<T, chap_vm::host::VmError>>,
    {
        match tokio::time::timeout(Duration::from_millis(timeout_ms), call).await {
            Ok(result) => result.map_err(Into::into),
            Err(_) => Err(vm::VmError::TimedOut),
        }
    }

    async fn create(
        &self,
        id: &VmIdentity,
        requested: RequestedVmConfig,
    ) -> Result<VmRef, vm::VmError> {
        let config = self.backend_config(requested)?;
        if self
            .backend_call(self.settings.calls.create_timeout_ms, self.backend.get(id))
            .await?
            .is_some()
        {
            return Err(vm::VmError::AlreadyExists);
        }
        let VmPrincipal::Plugin(plugin_id) = &id.principal else {
            return Err(vm::VmError::Denied(
                "host-owned VMs cannot be created through the plugin path".into(),
            ));
        };
        let reservation = self.reserve_vm(plugin_id)?;
        let vm = self
            .backend_call(
                self.settings.calls.create_timeout_ms,
                self.backend.create(id, &config),
            )
            .await?;
        reservation.commit();
        Ok(vm)
    }

    async fn get_or_create(
        &self,
        id: &VmIdentity,
        requested: RequestedVmConfig,
    ) -> Result<VmRef, vm::VmError> {
        let config = self.backend_config(requested)?;
        let exists = self
            .backend_call(self.settings.calls.create_timeout_ms, self.backend.get(id))
            .await?
            .is_some();
        let reservation = if exists {
            None
        } else {
            let VmPrincipal::Plugin(plugin_id) = &id.principal else {
                return Err(vm::VmError::Denied(
                    "host-owned VMs cannot be created through the plugin path".into(),
                ));
            };
            Some(self.reserve_vm(plugin_id)?)
        };
        let vm = self
            .backend_call(
                self.settings.calls.create_timeout_ms,
                self.backend.get_or_create(id, &config),
            )
            .await?;
        if let Some(reservation) = reservation {
            reservation.commit();
        }
        Ok(vm)
    }

    async fn exec(
        &self,
        vm: &VmRef,
        args: Vec<String>,
        cwd: Option<String>,
        stdin: Option<Vec<u8>>,
        timeout_ms: Option<u64>,
    ) -> Result<VmExecOutcome, vm::VmError> {
        let timeout_ms = Some(
            timeout_ms
                .unwrap_or(self.settings.calls.exec_timeout_ceiling_ms)
                .min(self.settings.calls.exec_timeout_ceiling_ms),
        );
        self.backend
            .exec(
                vm,
                VmCommand {
                    args,
                    cwd,
                    stdin,
                    timeout_ms,
                },
            )
            .await
            .map_err(Into::into)
    }

    async fn read_file(&self, vm: &VmRef, path: &str) -> Result<Vec<u8>, vm::VmError> {
        self.backend
            .read_file(vm, path, self.settings.calls.read_file_max_bytes)
            .await
            .map_err(Into::into)
    }

    async fn write_file(&self, vm: &VmRef, path: &str, contents: &[u8]) -> Result<(), vm::VmError> {
        self.backend
            .write_file(vm, path, contents)
            .await
            .map_err(Into::into)
    }

    async fn destroy(&self, plugin_id: &PluginId, vm: &ManagedVm) -> Result<(), vm::VmError> {
        let ManagedVm::CreatedByCaller { vm_ref } = vm else {
            return Err(vm::VmError::Denied(
                "the workspace is owned by the host".into(),
            ));
        };
        self.backend_call(
            self.settings.calls.destroy_timeout_ms,
            self.backend.destroy(vm_ref),
        )
        .await?;
        decrement_vm_count(&self.vm_counts, plugin_id);
        Ok(())
    }
}

struct VmCleanup<B: VmBackend> {
    backend: Arc<B>,
    installation_id: String,
    session_epoch: u64,
    timeout: Duration,
    shutdown_on_drop: bool,
}

impl<B: VmBackend> Drop for VmCleanup<B> {
    fn drop(&mut self) {
        if !self.shutdown_on_drop {
            return;
        }
        let backend = Arc::clone(&self.backend);
        let installation_id = self.installation_id.clone();
        let session_epoch = self.session_epoch;
        let timeout = self.timeout;
        let cleanup = std::thread::Builder::new()
            .name("chap-vm-cleanup".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())?;
                match runtime.block_on(async {
                    tokio::time::timeout(timeout, backend.shutdown(&installation_id, session_epoch))
                        .await
                }) {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(_) => Err("VM shutdown timed out".into()),
                }
            });
        match cleanup {
            Ok(cleanup) => match cleanup.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => warn_lifecycle_failure(SHUTDOWN_REAP_OPERATION, error),
                Err(_) => tracing::warn!("VM cleanup thread panicked"),
            },
            Err(error) => tracing::warn!(%error, "failed to start VM cleanup thread"),
        }
    }
}

fn session_epoch() -> u64 {
    static SESSION_EPOCH: OnceLock<u64> = OnceLock::new();
    *SESSION_EPOCH.get_or_init(|| {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        (seconds << 32) | u64::from(std::process::id())
    })
}

struct VmReservation {
    counts: Arc<Mutex<HashMap<PluginId, u32>>>,
    plugin_id: PluginId,
    active: bool,
}

impl VmReservation {
    fn commit(mut self) {
        self.active = false;
    }
}

impl Drop for VmReservation {
    fn drop(&mut self) {
        if self.active {
            decrement_vm_count(&self.counts, &self.plugin_id);
        }
    }
}

fn decrement_vm_count(counts: &Mutex<HashMap<PluginId, u32>>, plugin_id: &PluginId) {
    let mut counts = counts.lock().unwrap_or_else(PoisonError::into_inner);
    let remove = if let Some(count) = counts.get_mut(plugin_id) {
        *count = count.saturating_sub(1);
        *count == 0
    } else {
        false
    };
    if remove {
        counts.remove(plugin_id);
    }
}

impl ResolveScopedResource<chap_vm::vm::InstanceScope, String> for CapabilityHost {
    type Resource = ManagedVm;
    type Error = vm::VmError;

    async fn resolve_scoped_resource<'a>(
        &'a self,
        subject: &'a PluginSubject<'_>,
        name: &'a String,
    ) -> Result<Self::Resource, Self::Error> {
        if name == WORKSPACE_HANDLE {
            self.vm.resolve_workspace()
        } else {
            self.vm.resolve_owned(subject, name).await
        }
    }
}

#[lockgate::guarded]
impl vm::Host for CapabilityHost {
    #[lockgate::no_capability_required(
        reason = "answers from config without booting and performs its scoped vm.manage check by hand"
    )]
    async fn workspace(&mut self, cx: HostCtx<'_, ()>) -> Result<vm::WorkspaceInfo, vm::VmError> {
        let workspace = self.vm.workspace_record()?;
        cx.require_scoped(chap_vm::vm::MANAGE, &WorkspaceResource)?;
        Ok(workspace)
    }

    #[lockgate::no_capability_required(
        reason = "needs vm::create plus per-element vm::mount and vm::egress checks; the guard admits one classification per method (lockgate#21)"
    )]
    async fn create(
        &mut self,
        cx: HostCtx<'_, ()>,
        name: String,
        config: vm::VmConfig,
    ) -> Result<String, vm::VmError> {
        reject_reserved_name(&name)?;
        let (requested, mounts, egress) = translate_requested_config(&config, &self.vm.settings)?;
        cx.require(chap_vm::vm::CREATE)?;
        cx.require_scoped_each(chap_vm::vm::MOUNT, &mounts)?;
        cx.require_scoped_each(chap_vm::vm::EGRESS, &egress)?;
        let id = self.vm.identity(&cx.subject(), &name);
        self.vm.create(&id, requested).await?;
        Ok(name)
    }

    #[lockgate::no_capability_required(
        reason = "an absent target cannot be expressed as a successful result by the guard"
    )]
    async fn get(
        &mut self,
        cx: HostCtx<'_, ()>,
        name: String,
    ) -> Result<Option<String>, vm::VmError> {
        self.vm.validate_get_name(&name)?;
        match self.vm.resolve_owned(&cx.subject(), &name).await {
            Ok(vm) => {
                cx.require_scoped(chap_vm::vm::MANAGE, &vm)?;
                Ok(Some(name))
            }
            Err(vm::VmError::NoSuchVm) => Ok(None),
            Err(error) => Err(error),
        }
    }

    #[lockgate::no_capability_required(
        reason = "needs vm::create plus per-element vm::mount and vm::egress checks; the guard admits one classification per method (lockgate#21)"
    )]
    async fn get_or_create(
        &mut self,
        cx: HostCtx<'_, ()>,
        name: String,
        config: vm::VmConfig,
    ) -> Result<String, vm::VmError> {
        reject_reserved_name(&name)?;
        let (requested, mounts, egress) = translate_requested_config(&config, &self.vm.settings)?;
        cx.require(chap_vm::vm::CREATE)?;
        cx.require_scoped_each(chap_vm::vm::MOUNT, &mounts)?;
        cx.require_scoped_each(chap_vm::vm::EGRESS, &egress)?;
        let id = self.vm.identity(&cx.subject(), &name);
        self.vm.get_or_create(&id, requested).await?;
        Ok(name)
    }

    #[lockgate::requires(permission = chap_vm::vm::MANAGE, target = vm, wire_type = String)]
    async fn exec(
        &mut self,
        _cx: HostCtx<'_, ()>,
        vm: ManagedVm,
        command: Vec<String>,
        cwd: Option<String>,
        stdin: Option<Vec<u8>>,
        timeout_ms: Option<u64>,
    ) -> Result<vm::ExecResult, vm::VmError> {
        let vm_ref = self.vm.live_ref(&vm).await?;
        self.vm
            .exec(&vm_ref, command, cwd, stdin, timeout_ms)
            .await
            .map(Into::into)
    }

    #[lockgate::requires(permission = chap_vm::vm::MANAGE, target = vm, wire_type = String)]
    async fn write_file(
        &mut self,
        _cx: HostCtx<'_, ()>,
        vm: ManagedVm,
        path: String,
        contents: Vec<u8>,
    ) -> Result<(), vm::VmError> {
        let vm_ref = self.vm.live_ref(&vm).await?;
        self.vm.write_file(&vm_ref, &path, &contents).await
    }

    #[lockgate::requires(permission = chap_vm::vm::MANAGE, target = vm, wire_type = String)]
    async fn read_file(
        &mut self,
        _cx: HostCtx<'_, ()>,
        vm: ManagedVm,
        path: String,
    ) -> Result<Vec<u8>, vm::VmError> {
        let vm_ref = self.vm.live_ref(&vm).await?;
        self.vm.read_file(&vm_ref, &path).await
    }

    #[lockgate::requires(permission = chap_vm::vm::MANAGE, target = vm, wire_type = String)]
    async fn destroy(&mut self, cx: HostCtx<'_, ()>, vm: ManagedVm) -> Result<(), vm::VmError> {
        self.vm.destroy(cx.subject().plugin_id(), &vm).await
    }
}

impl From<PermissionDenied> for vm::VmError {
    fn from(error: PermissionDenied) -> Self {
        Self::Denied(format!("{}.{}", error.capability(), error.permission()))
    }
}

impl From<chap_vm::host::VmError> for vm::VmError {
    fn from(error: chap_vm::host::VmError) -> Self {
        translate_host_error(error)
    }
}

impl From<VmExecOutcome> for vm::ExecResult {
    fn from(outcome: VmExecOutcome) -> Self {
        Self {
            exit_code: outcome.exit_code,
            stdout: outcome.stdout,
            stderr: outcome.stderr,
            truncated: outcome.truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chap_vm::host::{EnvVar, MountSpec, OciReference, ResolvedImage, VmCallSettings, VmLimits};
    use std::{
        collections::HashSet,
        io::{self, Write},
        path::Path,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use tokio::sync::Notify;

    #[derive(Clone)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    impl Write for CapturedLogs {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct ControlledBackend {
        inner: Backend,
        blocked_plugin_ids: HashSet<PluginId>,
        failed_identities: HashSet<(PluginId, String)>,
        create_started: Notify,
        release_create: Notify,
        destroy_never_completes: AtomicBool,
        vanish_on_owner: AtomicBool,
        reap_fails: AtomicBool,
        block_workspace_create: AtomicBool,
        fail_workspace_create_once: AtomicBool,
        workspace_create_started: Notify,
        release_workspace_create: Notify,
        workspace_get_or_create_calls: AtomicUsize,
        workspace_configs: Mutex<Vec<VmConfig>>,
    }

    impl ControlledBackend {
        fn new(blocked_plugin_ids: &[&str], failed_identities: &[(&str, &str)]) -> Self {
            Self {
                inner: Backend::new(&VmSettings::default()),
                blocked_plugin_ids: blocked_plugin_ids
                    .iter()
                    .map(|plugin_id| plugin_id.parse().unwrap())
                    .collect(),
                failed_identities: failed_identities
                    .iter()
                    .map(|(plugin_id, name)| (plugin_id.parse().unwrap(), (*name).to_owned()))
                    .collect(),
                create_started: Notify::new(),
                release_create: Notify::new(),
                destroy_never_completes: AtomicBool::new(false),
                vanish_on_owner: AtomicBool::new(false),
                reap_fails: AtomicBool::new(false),
                block_workspace_create: AtomicBool::new(false),
                fail_workspace_create_once: AtomicBool::new(false),
                workspace_create_started: Notify::new(),
                release_workspace_create: Notify::new(),
                workspace_get_or_create_calls: AtomicUsize::new(0),
                workspace_configs: Mutex::new(Vec::new()),
            }
        }

        fn vanish_on_next_owner_lookup(&self) {
            self.vanish_on_owner.store(true, Ordering::SeqCst);
        }

        fn block_destroy(&self) {
            self.destroy_never_completes.store(true, Ordering::SeqCst);
        }

        fn fail_reap(&self) {
            self.reap_fails.store(true, Ordering::SeqCst);
        }

        fn block_workspace_create(&self) {
            self.block_workspace_create.store(true, Ordering::SeqCst);
        }

        fn fail_next_workspace_create(&self) {
            self.fail_workspace_create_once
                .store(true, Ordering::SeqCst);
        }
    }

    impl VmBackend for ControlledBackend {
        async fn create(
            &self,
            id: &VmIdentity,
            cfg: &VmConfig,
        ) -> Result<VmRef, chap_vm::host::VmError> {
            let plugin_id = match &id.principal {
                VmPrincipal::Plugin(plugin_id) => Some(plugin_id),
                VmPrincipal::Host => None,
            };
            if plugin_id.is_some_and(|plugin_id| self.blocked_plugin_ids.contains(plugin_id)) {
                self.create_started.notify_one();
                self.release_create.notified().await;
            }
            if plugin_id.is_some_and(|plugin_id| {
                self.failed_identities
                    .contains(&(plugin_id.clone(), id.logical_name.clone()))
            }) {
                return Err(chap_vm::host::VmError::Failed(
                    "injected create failure".into(),
                ));
            }
            self.inner.create(id, cfg).await
        }

        async fn get(&self, id: &VmIdentity) -> Result<Option<VmRef>, chap_vm::host::VmError> {
            self.inner.get(id).await
        }

        async fn get_or_create(
            &self,
            id: &VmIdentity,
            cfg: &VmConfig,
        ) -> Result<VmRef, chap_vm::host::VmError> {
            if id.principal == VmPrincipal::Host {
                self.workspace_get_or_create_calls
                    .fetch_add(1, Ordering::SeqCst);
                self.workspace_configs.lock().unwrap().push(cfg.clone());
                if self
                    .fail_workspace_create_once
                    .swap(false, Ordering::SeqCst)
                {
                    return Err(chap_vm::host::VmError::Failed(
                        "injected workspace create failure".into(),
                    ));
                }
                if self.block_workspace_create.load(Ordering::SeqCst) {
                    self.workspace_create_started.notify_one();
                    self.release_workspace_create.notified().await;
                }
            }
            self.inner.get_or_create(id, cfg).await
        }

        async fn exec(
            &self,
            vm: &VmRef,
            command: VmCommand,
        ) -> Result<VmExecOutcome, chap_vm::host::VmError> {
            self.inner.exec(vm, command).await
        }

        async fn read_file(
            &self,
            vm: &VmRef,
            path: &str,
            max_bytes: u64,
        ) -> Result<Vec<u8>, chap_vm::host::VmError> {
            self.inner.read_file(vm, path, max_bytes).await
        }

        async fn write_file(
            &self,
            vm: &VmRef,
            path: &str,
            bytes: &[u8],
        ) -> Result<(), chap_vm::host::VmError> {
            self.inner.write_file(vm, path, bytes).await
        }

        async fn destroy(&self, vm: &VmRef) -> Result<(), chap_vm::host::VmError> {
            if self.destroy_never_completes.load(Ordering::SeqCst) {
                return std::future::pending().await;
            }
            self.inner.destroy(vm).await
        }

        async fn owner_of(
            &self,
            vm: &VmRef,
        ) -> Result<Option<VmPrincipal>, chap_vm::host::VmError> {
            if self.vanish_on_owner.swap(false, Ordering::SeqCst) {
                self.inner.destroy(vm).await?;
            }
            self.inner.owner_of(vm).await
        }

        async fn reap(
            &self,
            installation_id: &str,
            current_epoch: u64,
        ) -> Result<(), chap_vm::host::VmError> {
            if self.reap_fails.load(Ordering::SeqCst) {
                return Err(chap_vm::host::VmError::Failed(
                    "injected reap failure".into(),
                ));
            }
            self.inner.reap(installation_id, current_epoch).await
        }

        async fn shutdown(
            &self,
            installation_id: &str,
            session_epoch: u64,
        ) -> Result<(), chap_vm::host::VmError> {
            self.inner.shutdown(installation_id, session_epoch).await
        }
    }

    async fn test_host(
        backend: Arc<ControlledBackend>,
        max_vms_per_plugin: u32,
    ) -> VmHost<ControlledBackend> {
        test_host_with_workspace(backend, max_vms_per_plugin, None).await
    }

    async fn test_host_with_workspace(
        backend: Arc<ControlledBackend>,
        max_vms_per_plugin: u32,
        workspace: Option<ResolvedWorkspace>,
    ) -> VmHost<ControlledBackend> {
        VmHost::with_backend(
            backend,
            VmSettings {
                registries: vec!["ghcr.io".into()],
                limits: VmLimits { max_vms_per_plugin },
                calls: VmCallSettings {
                    exec_timeout_ceiling_ms: 1_000,
                    ..VmCallSettings::default()
                },
                ..VmSettings::default()
            },
            "test-installation".into(),
            1,
            workspace,
        )
        .await
        .unwrap()
    }

    fn identity(plugin_id: &str, logical_name: &str) -> VmIdentity {
        VmIdentity {
            installation_id: "test-installation".into(),
            session_epoch: 1,
            principal: VmPrincipal::Plugin(plugin_id.parse().unwrap()),
            logical_name: logical_name.into(),
        }
    }

    fn requested_config() -> RequestedVmConfig {
        RequestedVmConfig::normalized(
            OciReference::parse("ghcr.io/acme/build:1.2").unwrap(),
            Vec::<MountSpec>::new(),
            Vec::new(),
            Vec::<EnvVar>::new(),
            Vec::new(),
        )
        .unwrap()
    }

    fn resolved_workspace() -> ResolvedWorkspace {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        let requested = RequestedVmConfig::normalized(
            OciReference::parse("ghcr.io/acme/build:1.2").unwrap(),
            vec![MountSpec {
                host: directory.to_string_lossy().into_owned(),
                guest: "/mnt/workspace".into(),
                readonly: true,
            }],
            vec!["127.0.0.1:1".parse().unwrap()],
            Vec::new(),
            vec![chap_vm::host::SecretSpec {
                env: "GITHUB_TOKEN".into(),
                source: chap_vm::host::SecretSource::HostEnv("CHAP_GITHUB_TOKEN".into()),
                hosts: vec!["api.github.com".into(), "github.com".into()],
            }],
        )
        .unwrap();
        ResolvedWorkspace {
            info: crate::agent::WorkspaceInfo {
                directory,
                readonly: true,
                image: "ghcr.io/acme/build:1.2".into(),
                egress: vec!["127.0.0.1:1".into()],
                secrets: vec![crate::agent::WorkspaceSecret {
                    env: "GITHUB_TOKEN".into(),
                    hosts: vec!["api.github.com".into(), "github.com".into()],
                }],
            },
            requested,
        }
    }

    fn resolved_config() -> VmConfig {
        VmConfig {
            image: ResolvedImage {
                registry: "ghcr.io".into(),
                repository: "acme/build".into(),
                tag: Some("1.2".into()),
                digest: None,
            },
            mounts: Vec::new(),
            egress: Vec::new(),
            env: Vec::new(),
            secrets: Vec::new(),
            cpus: 1,
            memory_mb: 512,
            max_lifetime_ms: 3_600_000,
            idle_timeout_ms: 300_000,
            config_hash: "test-config".into(),
        }
    }

    #[test]
    fn at_prefix_is_reserved_for_host_handles() {
        for name in ["@workspace", "@host", "@"] {
            assert!(matches!(
                reject_reserved_name(name),
                Err(vm::VmError::Denied(message)) if message.contains("reserved prefix")
            ));
        }
        assert!(reject_reserved_name("workspace").is_ok());
    }

    #[tokio::test]
    async fn workspace_use_fails_when_no_workspace_is_configured() {
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        let host = test_host(backend, 1).await;

        assert!(matches!(
            host.resolve_workspace(),
            Err(vm::VmError::Failed(message)) if message.contains("no workspace is configured")
        ));
        assert!(matches!(
            host.validate_get_name(WORKSPACE_HANDLE),
            Err(vm::VmError::Failed(message)) if message.contains("no workspace is configured")
        ));
        assert!(matches!(
            host.workspace_record(),
            Err(vm::VmError::Failed(message)) if message.contains("no workspace is configured")
        ));
    }

    #[tokio::test]
    async fn workspace_record_reports_config_without_booting_the_vm() {
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        let host =
            test_host_with_workspace(Arc::clone(&backend), 1, Some(resolved_workspace())).await;

        let workspace = host.workspace_record().unwrap();
        assert_eq!(workspace.vm, "@workspace");
        assert_eq!(workspace.image, "ghcr.io/acme/build:1.2");
        assert_eq!(workspace.mounts.len(), 1);
        assert_eq!(workspace.mounts[0].guest, "/mnt/workspace");
        assert!(workspace.mounts[0].readonly);
        assert_eq!(workspace.egress, ["127.0.0.1:1"]);
        assert_eq!(workspace.secrets.len(), 1);
        assert_eq!(workspace.secrets[0].env, "GITHUB_TOKEN");
        assert_eq!(workspace.secrets[0].hosts, ["api.github.com", "github.com"]);
        assert_eq!(
            backend.workspace_get_or_create_calls.load(Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn destroying_the_workspace_does_not_boot_it() {
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        let host =
            test_host_with_workspace(Arc::clone(&backend), 1, Some(resolved_workspace())).await;
        let workspace = host.resolve_workspace().unwrap();
        let plugin_id = "caller".parse().unwrap();

        assert!(matches!(
            host.destroy(&plugin_id, &workspace).await,
            Err(vm::VmError::Denied(message)) if message == "the workspace is owned by the host"
        ));
        assert_eq!(
            backend.workspace_get_or_create_calls.load(Ordering::SeqCst),
            0
        );
        assert!(host.workspace_vm.get().is_none());
    }

    #[tokio::test]
    async fn failed_workspace_boot_leaves_the_lazy_cell_empty_for_a_retry() {
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        backend.fail_next_workspace_create();
        let host =
            test_host_with_workspace(Arc::clone(&backend), 1, Some(resolved_workspace())).await;
        let workspace = host.resolve_workspace().unwrap();

        assert!(matches!(
            host.live_ref(&workspace).await,
            Err(vm::VmError::Failed(message)) if message.contains("injected workspace create failure")
        ));
        assert!(host.workspace_vm.get().is_none());

        let vm_ref = host.live_ref(&workspace).await.unwrap();
        assert_eq!(
            backend.workspace_get_or_create_calls.load(Ordering::SeqCst),
            2
        );
        assert_eq!(host.workspace_vm.get(), Some(&vm_ref));
    }

    #[tokio::test]
    async fn concurrent_workspace_use_creates_one_host_owned_vm_outside_plugin_limits() {
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        backend.block_workspace_create();
        let host =
            test_host_with_workspace(Arc::clone(&backend), 0, Some(resolved_workspace())).await;
        let first_host = host.clone();
        let first = tokio::spawn(async move {
            let workspace = first_host.resolve_workspace()?;
            first_host.live_ref(&workspace).await
        });
        backend.workspace_create_started.notified().await;

        let second_host = host.clone();
        let second = tokio::spawn(async move {
            let workspace = second_host.resolve_workspace()?;
            second_host.live_ref(&workspace).await
        });
        tokio::task::yield_now().await;
        assert_eq!(
            backend.workspace_get_or_create_calls.load(Ordering::SeqCst),
            1
        );

        backend.release_workspace_create.notify_one();
        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert_eq!(first, second);
        assert!(host.vm_counts.lock().unwrap().is_empty());
        assert_eq!(
            backend.owner_of(&first).await.unwrap(),
            Some(VmPrincipal::Host)
        );
        assert!(matches!(
            host.validate_get_name(WORKSPACE_HANDLE),
            Err(vm::VmError::Denied(message)) if message == "the workspace is owned by the host"
        ));

        {
            let configs = backend.workspace_configs.lock().unwrap();
            assert_eq!(configs.len(), 1);
            assert_eq!(configs[0].mounts.len(), 1);
            assert_eq!(configs[0].mounts[0].guest, "/mnt/workspace");
            assert!(configs[0].mounts[0].readonly);
            assert_eq!(configs[0].image.registry, "ghcr.io");
            assert_eq!(configs[0].image.repository, "acme/build");
            assert_eq!(configs[0].egress, vec!["127.0.0.1:1".parse().unwrap()]);
            assert!(configs[0].env.is_empty());
            assert_eq!(
                configs[0].secrets,
                [chap_vm::host::SecretSpec {
                    env: "GITHUB_TOKEN".into(),
                    source: chap_vm::host::SecretSource::HostEnv("CHAP_GITHUB_TOKEN".into()),
                    hosts: vec!["api.github.com".into(), "github.com".into()],
                }]
            );
            assert_eq!(configs[0].cpus, 1);
            assert_eq!(configs[0].memory_mb, 512);
            assert_eq!(configs[0].max_lifetime_ms, 3_600_000);
            assert_eq!(configs[0].idle_timeout_ms, 300_000);
        }

        assert!(
            backend
                .get(&host.workspace_identity())
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn caller_owned_resolution_never_returns_the_host_workspace() {
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        let host =
            test_host_with_workspace(Arc::clone(&backend), 1, Some(resolved_workspace())).await;
        host.workspace_ref().await.unwrap();
        let plugin_id = "caller".parse().unwrap();

        assert!(matches!(
            host.resolve_owned_identity(&host.workspace_identity(), &plugin_id)
                .await,
            Err(vm::VmError::NoSuchVm)
        ));
    }

    #[tokio::test]
    async fn slow_creates_do_not_block_other_plugins_and_failures_release_the_count() {
        let backend = Arc::new(ControlledBackend::new(&["slow"], &[("failing", "first")]));
        let host = test_host(Arc::clone(&backend), 1).await;
        let slow_host = host.clone();
        let slow = tokio::spawn(async move {
            slow_host
                .create(&identity("slow", "first"), requested_config())
                .await
        });
        backend.create_started.notified().await;

        let fast = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            host.create(&identity("fast", "first"), requested_config()),
        )
        .await
        .expect("another plugin must not wait for the slow backend call");
        assert!(fast.is_ok());

        backend.release_create.notify_one();
        slow.await.unwrap().unwrap();

        assert!(
            host.create(&identity("failing", "first"), requested_config())
                .await
                .is_err()
        );
        assert!(
            host.create(&identity("failing", "second"), requested_config())
                .await
                .is_ok()
        );
        assert_eq!(
            host.vm_counts
                .lock()
                .unwrap()
                .get(&"failing".parse().unwrap())
                .copied(),
            Some(1)
        );
    }

    #[tokio::test]
    async fn create_timeout_is_independent_from_the_exec_ceiling() {
        let timeout_backend = Arc::new(ControlledBackend::new(&["timeout"], &[]));
        let mut timeout_host = test_host(Arc::clone(&timeout_backend), 1).await;
        timeout_host.settings.calls.create_timeout_ms = 10;
        assert_eq!(timeout_host.settings.calls.exec_timeout_ceiling_ms, 1_000);
        assert!(matches!(
            timeout_host
                .create(&identity("timeout", "first"), requested_config())
                .await,
            Err(vm::VmError::TimedOut)
        ));
        timeout_backend.release_create.notify_one();
        assert!(
            timeout_host
                .create(&identity("timeout", "second"), requested_config())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn plugin_destroy_times_out_when_the_backend_never_completes() {
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        let mut host = test_host(Arc::clone(&backend), 1).await;
        let id = identity("principal", "vm");
        let vm = host.create(&id, requested_config()).await.unwrap();
        backend.block_destroy();
        host.settings.calls.destroy_timeout_ms = 10;
        let VmPrincipal::Plugin(plugin_id) = &id.principal else {
            unreachable!()
        };

        let vm = ManagedVm::CreatedByCaller { vm_ref: vm };
        assert!(matches!(
            host.destroy(plugin_id, &vm).await,
            Err(vm::VmError::TimedOut)
        ));
        assert_eq!(
            host.vm_counts.lock().unwrap().get(plugin_id).copied(),
            Some(1)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn host_construction_warns_and_continues_when_startup_reap_fails() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::WARN)
            .with_writer({
                let captured = Arc::clone(&captured);
                move || CapturedLogs(Arc::clone(&captured))
            })
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        backend.fail_reap();

        let host = VmHost::with_backend(
            backend,
            VmSettings {
                registries: vec!["ghcr.io".into()],
                calls: VmCallSettings {
                    exec_timeout_ceiling_ms: 1_000,
                    ..VmCallSettings::default()
                },
                ..VmSettings::default()
            },
            "test-installation".into(),
            2,
            None,
        )
        .await;

        assert!(host.is_ok());
        let output = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        assert!(output.contains("WARN"), "{output}");
        assert!(output.contains("VM lifecycle operation failed"), "{output}");
        assert!(
            output.contains("operation=\"reap stale VMs at startup\""),
            "{output}"
        );
        assert!(output.contains("error=injected reap failure"), "{output}");
    }

    #[tokio::test]
    async fn host_lifecycle_reaps_stale_vms_and_destroys_only_its_epoch_on_drop() {
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        let stale = VmIdentity {
            session_epoch: 1,
            ..identity("stale", "vm")
        };
        let current = VmIdentity {
            session_epoch: 2,
            ..identity("current", "vm")
        };
        let other_installation = VmIdentity {
            installation_id: "other-installation".into(),
            session_epoch: 1,
            ..identity("other", "vm")
        };
        for id in [&stale, &current, &other_installation] {
            backend.inner.create(id, &resolved_config()).await.unwrap();
        }

        let host = VmHost::with_backend(
            Arc::clone(&backend),
            VmSettings {
                registries: vec!["ghcr.io".into()],
                calls: VmCallSettings {
                    exec_timeout_ceiling_ms: 1_000,
                    ..VmCallSettings::default()
                },
                ..VmSettings::default()
            },
            "test-installation".into(),
            current.session_epoch,
            None,
        )
        .await
        .unwrap();

        assert_eq!(backend.inner.get(&stale).await.unwrap(), None);
        assert!(backend.inner.get(&current).await.unwrap().is_some());
        assert!(
            backend
                .inner
                .get(&other_installation)
                .await
                .unwrap()
                .is_some()
        );

        drop(host);

        assert_eq!(backend.inner.get(&current).await.unwrap(), None);
        assert!(
            backend
                .inner
                .get(&other_installation)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn an_unknown_logical_name_does_not_resolve() {
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        let host = test_host(backend, 1).await;
        let plugin_id = "principal".parse().unwrap();

        let result = host
            .resolve_owned_identity(&identity("principal", "missing"), &plugin_id)
            .await;

        assert!(matches!(result, Err(vm::VmError::NoSuchVm)));
    }

    #[tokio::test]
    async fn a_vm_created_by_the_caller_resolves_to_the_backend_reference() {
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        let host = test_host(Arc::clone(&backend), 1).await;
        let id = identity("principal", "owned");
        let expected = backend.create(&id, &resolved_config()).await.unwrap();
        let plugin_id = "principal".parse().unwrap();

        let managed = host.resolve_owned_identity(&id, &plugin_id).await.unwrap();

        assert!(matches!(
            managed,
            ManagedVm::CreatedByCaller { vm_ref } if vm_ref == expected
        ));
    }

    #[tokio::test]
    async fn a_vm_that_vanishes_during_ownership_lookup_is_not_found() {
        let backend = Arc::new(ControlledBackend::new(&[], &[]));
        let host = test_host(Arc::clone(&backend), 1).await;
        let id = identity("principal", "vanishing");
        backend.create(&id, &resolved_config()).await.unwrap();
        backend.vanish_on_next_owner_lookup();

        let plugin_id = "principal".parse().unwrap();
        let result = host.resolve_owned_identity(&id, &plugin_id).await;

        assert!(matches!(result, Err(vm::VmError::NoSuchVm)));
    }
}
