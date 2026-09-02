use super::{CapabilityHost, vm};
use crate::{
    agent::vm_host::{ManagedVm, translate_host_error, translate_requested_config},
    config::Config,
    consent::ConsentError,
};
use chap_vm::host::{
    Backend, ExecOutcome as VmExecOutcome, RequestedVmConfig, Subject, VmBackend, VmCommand,
    VmConfig, VmIdentity, VmRef, VmSettings,
};
use lockgate::{HostCtx, PermissionDenied, PluginSubject};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(crate) struct VmHost<B: VmBackend = Backend> {
    pub(crate) backend: Arc<B>,
    pub(crate) settings: VmSettings,
    pub(crate) installation_id: String,
    pub(crate) session_epoch: u64,
    pub(crate) vm_counts: Arc<Mutex<HashMap<String, u32>>>,
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
            cleanup: Arc::clone(&self.cleanup),
        }
    }
}

pub(in crate::agent) async fn new(config: &Config) -> Result<VmHost, ConsentError> {
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
    )
    .await
    .map_err(|source| ConsentError::VmLifecycle {
        operation: "reap stale VMs",
        source,
    })
}

impl<B: VmBackend> VmHost<B> {
    async fn with_backend(
        backend: Arc<B>,
        settings: VmSettings,
        installation_id: String,
        session_epoch: u64,
    ) -> Result<Self, chap_vm::host::VmError> {
        match tokio::time::timeout(
            Duration::from_millis(settings.max_exec_ms),
            backend.reap(&installation_id, session_epoch),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => return Err(chap_vm::host::VmError::TimedOut),
        }
        Ok(Self {
            cleanup: Arc::new(VmCleanup {
                backend: Arc::clone(&backend),
                installation_id: installation_id.clone(),
                session_epoch,
                timeout: Duration::from_millis(settings.max_exec_ms),
            }),
            backend,
            settings,
            installation_id,
            session_epoch,
            vm_counts: Arc::default(),
        })
    }

    pub(crate) fn identity(&self, subject: &PluginSubject<'_>, logical_name: &str) -> VmIdentity {
        VmIdentity {
            installation_id: self.installation_id.clone(),
            session_epoch: self.session_epoch,
            principal: subject.plugin_id().to_owned(),
            logical_name: logical_name.to_owned(),
        }
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
            cpus: self.settings.default_cpus,
            memory_mb: self.settings.default_memory_mb,
            max_duration_ms: self.settings.max_duration_ms,
            idle_timeout_ms: self.settings.idle_timeout_ms,
            config_hash,
        })
    }

    fn enforce_vm_limit(&self, count: u32) -> Result<(), vm::VmError> {
        if count >= self.settings.max_vms_per_plugin {
            Err(vm::VmError::Denied(format!(
                "maximum of {} VMs per plugin reached",
                self.settings.max_vms_per_plugin
            )))
        } else {
            Ok(())
        }
    }

    fn counts(&self) -> MutexGuard<'_, HashMap<String, u32>> {
        self.vm_counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn reserve_vm(&self, principal: &str) -> Result<VmReservation, vm::VmError> {
        let mut counts = self.counts();
        let count = counts.get(principal).copied().unwrap_or_default();
        self.enforce_vm_limit(count)?;
        counts.insert(principal.to_owned(), count + 1);
        drop(counts);
        Ok(VmReservation {
            counts: Arc::clone(&self.vm_counts),
            principal: principal.to_owned(),
            active: true,
        })
    }

    async fn backend_call<T, F>(&self, call: F) -> Result<T, vm::VmError>
    where
        F: Future<Output = Result<T, chap_vm::host::VmError>>,
    {
        match tokio::time::timeout(Duration::from_millis(self.settings.max_exec_ms), call).await {
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
        if self.backend_call(self.backend.get(id)).await?.is_some() {
            return Err(vm::VmError::AlreadyExists);
        }
        let reservation = self.reserve_vm(&id.principal)?;
        let vm = self.backend_call(self.backend.create(id, &config)).await?;
        reservation.commit();
        Ok(vm)
    }

    async fn get_or_create(
        &self,
        id: &VmIdentity,
        requested: RequestedVmConfig,
    ) -> Result<VmRef, vm::VmError> {
        let config = self.backend_config(requested)?;
        let exists = self.backend_call(self.backend.get(id)).await?.is_some();
        let reservation = if exists {
            None
        } else {
            Some(self.reserve_vm(&id.principal)?)
        };
        let vm = self
            .backend_call(self.backend.get_or_create(id, &config))
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
                .unwrap_or(self.settings.max_exec_ms)
                .min(self.settings.max_exec_ms),
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
            .read_file(vm, path, self.settings.max_read_bytes)
            .await
            .map_err(Into::into)
    }

    async fn write_file(&self, vm: &VmRef, path: &str, contents: &[u8]) -> Result<(), vm::VmError> {
        self.backend
            .write_file(vm, path, contents)
            .await
            .map_err(Into::into)
    }

    async fn destroy(&self, principal: &str, vm: &VmRef) -> Result<(), vm::VmError> {
        self.backend_call(self.backend.destroy(vm)).await?;
        decrement_vm_count(&self.vm_counts, principal);
        Ok(())
    }
}

struct VmCleanup<B: VmBackend> {
    backend: Arc<B>,
    installation_id: String,
    session_epoch: u64,
    timeout: Duration,
}

impl<B: VmBackend> Drop for VmCleanup<B> {
    fn drop(&mut self) {
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
                Ok(Err(error)) => tracing::warn!(%error, "failed to clean up session VMs"),
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
    counts: Arc<Mutex<HashMap<String, u32>>>,
    principal: String,
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
            decrement_vm_count(&self.counts, &self.principal);
        }
    }
}

fn decrement_vm_count(counts: &Mutex<HashMap<String, u32>>, principal: &str) {
    let mut counts = counts.lock().unwrap_or_else(PoisonError::into_inner);
    let remove = if let Some(count) = counts.get_mut(principal) {
        *count = count.saturating_sub(1);
        *count == 0
    } else {
        false
    };
    if remove {
        counts.remove(principal);
    }
}

impl CapabilityHost {
    fn authorize_manage(
        &self,
        cx: &HostCtx<'_, ()>,
        owned_by_caller: bool,
    ) -> Result<(), vm::VmError> {
        cx.require_scoped(chap_vm::vm::MANAGE, &ManagedVm { owned_by_caller })
            .map_err(Into::into)
    }

    async fn authorize_owned(
        &self,
        cx: &HostCtx<'_, ()>,
        name: &str,
    ) -> Result<VmRef, vm::VmError> {
        let id = self.vm.identity(&cx.subject(), name);
        let Some(vm) = self.vm.backend.get(&id).await? else {
            // Physical identity includes the caller principal, so absence reveals no other VM.
            return Err(vm::VmError::NoSuchVm);
        };
        let subject = Subject(cx.subject().plugin_id().to_owned());
        if self.vm.backend.owner_of(&vm).await? != Some(subject) {
            self.authorize_manage(cx, false)?;
            return Err(vm::VmError::NoSuchVm);
        }
        self.authorize_manage(cx, true)?;
        Ok(vm)
    }
}

// Creation combines three guards, while management needs an async identity lookup first.
#[lockgate::guarded]
impl vm::Host for CapabilityHost {
    #[lockgate::no_capability_required(
        reason = "enforces vm::create plus per-element vm::mount and vm::egress"
    )]
    async fn create(
        &mut self,
        cx: HostCtx<'_, ()>,
        name: String,
        config: vm::VmConfig,
    ) -> Result<String, vm::VmError> {
        let (requested, mounts, egress) = translate_requested_config(&config, &self.vm.settings)?;
        cx.require(chap_vm::vm::CREATE)?;
        cx.require_scoped_each(chap_vm::vm::MOUNT, &mounts)?;
        cx.require_scoped_each(chap_vm::vm::EGRESS, &egress)?;
        let id = self.vm.identity(&cx.subject(), &name);
        self.vm.create(&id, requested).await?;
        Ok(name)
    }

    #[lockgate::no_capability_required(
        reason = "returns only the caller's own vm by physical identity"
    )]
    async fn get(
        &mut self,
        cx: HostCtx<'_, ()>,
        name: String,
    ) -> Result<Option<String>, vm::VmError> {
        let id = self.vm.identity(&cx.subject(), &name);
        match self.vm.backend.get(&id).await? {
            None => Ok(None),
            Some(_) => {
                self.authorize_manage(&cx, true)?;
                Ok(Some(name))
            }
        }
    }

    #[lockgate::no_capability_required(
        reason = "enforces vm::create plus per-element vm::mount and vm::egress"
    )]
    async fn get_or_create(
        &mut self,
        cx: HostCtx<'_, ()>,
        name: String,
        config: vm::VmConfig,
    ) -> Result<String, vm::VmError> {
        let (requested, mounts, egress) = translate_requested_config(&config, &self.vm.settings)?;
        cx.require(chap_vm::vm::CREATE)?;
        cx.require_scoped_each(chap_vm::vm::MOUNT, &mounts)?;
        cx.require_scoped_each(chap_vm::vm::EGRESS, &egress)?;
        let id = self.vm.identity(&cx.subject(), &name);
        self.vm.get_or_create(&id, requested).await?;
        Ok(name)
    }

    #[lockgate::no_capability_required(reason = "enforces vm::manage on the caller's own vm")]
    async fn exec(
        &mut self,
        cx: HostCtx<'_, ()>,
        name: String,
        command: Vec<String>,
        cwd: Option<String>,
        stdin: Option<Vec<u8>>,
        timeout_ms: Option<u64>,
    ) -> Result<vm::ExecResult, vm::VmError> {
        let vm = self.authorize_owned(&cx, &name).await?;
        self.vm
            .exec(&vm, command, cwd, stdin, timeout_ms)
            .await
            .map(Into::into)
    }

    #[lockgate::no_capability_required(reason = "enforces vm::manage on the caller's own vm")]
    async fn write_file(
        &mut self,
        cx: HostCtx<'_, ()>,
        name: String,
        path: String,
        contents: Vec<u8>,
    ) -> Result<(), vm::VmError> {
        let vm = self.authorize_owned(&cx, &name).await?;
        self.vm.write_file(&vm, &path, &contents).await
    }

    #[lockgate::no_capability_required(reason = "enforces vm::manage on the caller's own vm")]
    async fn read_file(
        &mut self,
        cx: HostCtx<'_, ()>,
        name: String,
        path: String,
    ) -> Result<Vec<u8>, vm::VmError> {
        let vm = self.authorize_owned(&cx, &name).await?;
        self.vm.read_file(&vm, &path).await
    }

    #[lockgate::no_capability_required(reason = "enforces vm::manage on the caller's own vm")]
    async fn destroy(&mut self, cx: HostCtx<'_, ()>, name: String) -> Result<(), vm::VmError> {
        let vm = self.authorize_owned(&cx, &name).await?;
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
    use chap_vm::host::{EnvVar, MountSpec, OciReference, ResolvedImage};
    use std::collections::HashSet;
    use tokio::sync::Notify;

    struct ControlledBackend {
        inner: Backend,
        blocked_principals: HashSet<String>,
        failed_identities: HashSet<(String, String)>,
        create_started: Notify,
        release_create: Notify,
    }

    impl ControlledBackend {
        fn new(blocked_principals: &[&str], failed_identities: &[(&str, &str)]) -> Self {
            Self {
                inner: Backend::new(&VmSettings::default()),
                blocked_principals: blocked_principals
                    .iter()
                    .map(|principal| (*principal).to_owned())
                    .collect(),
                failed_identities: failed_identities
                    .iter()
                    .map(|(principal, name)| ((*principal).to_owned(), (*name).to_owned()))
                    .collect(),
                create_started: Notify::new(),
                release_create: Notify::new(),
            }
        }
    }

    impl VmBackend for ControlledBackend {
        async fn create(
            &self,
            id: &VmIdentity,
            cfg: &VmConfig,
        ) -> Result<VmRef, chap_vm::host::VmError> {
            if self.blocked_principals.contains(&id.principal) {
                self.create_started.notify_one();
                self.release_create.notified().await;
            }
            if self
                .failed_identities
                .contains(&(id.principal.clone(), id.logical_name.clone()))
            {
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
            self.inner.destroy(vm).await
        }

        async fn owner_of(&self, vm: &VmRef) -> Result<Option<Subject>, chap_vm::host::VmError> {
            self.inner.owner_of(vm).await
        }

        async fn reap(
            &self,
            installation_id: &str,
            current_epoch: u64,
        ) -> Result<(), chap_vm::host::VmError> {
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
        VmHost::with_backend(
            backend,
            VmSettings {
                max_vms_per_plugin,
                max_exec_ms: 1_000,
                registries: vec!["ghcr.io".into()],
                ..VmSettings::default()
            },
            "test-installation".into(),
            1,
        )
        .await
        .unwrap()
    }

    fn identity(principal: &str, logical_name: &str) -> VmIdentity {
        VmIdentity {
            installation_id: "test-installation".into(),
            session_epoch: 1,
            principal: principal.into(),
            logical_name: logical_name.into(),
        }
    }

    fn requested_config() -> RequestedVmConfig {
        RequestedVmConfig::normalized(
            OciReference::parse("ghcr.io/acme/build:1.2").unwrap(),
            Vec::<MountSpec>::new(),
            Vec::new(),
            Vec::<EnvVar>::new(),
        )
        .unwrap()
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
            cpus: 1,
            memory_mb: 512,
            max_duration_ms: 3_600_000,
            idle_timeout_ms: 300_000,
            config_hash: "test-config".into(),
        }
    }

    #[tokio::test]
    async fn slow_creates_do_not_block_other_principals_and_failures_release_the_count() {
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
        .expect("another principal must not wait for the slow backend call");
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
            host.vm_counts.lock().unwrap().get("failing").copied(),
            Some(1)
        );

        let timeout_backend = Arc::new(ControlledBackend::new(&["timeout"], &[]));
        let mut timeout_host = test_host(Arc::clone(&timeout_backend), 1).await;
        timeout_host.settings.max_exec_ms = 10;
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
                max_exec_ms: 1_000,
                registries: vec!["ghcr.io".into()],
                ..VmSettings::default()
            },
            "test-installation".into(),
            current.session_epoch,
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
}
