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
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub(crate) struct VmHost {
    pub(crate) backend: Arc<Backend>,
    pub(crate) settings: VmSettings,
    pub(crate) installation_id: String,
    pub(crate) session_epoch: u64,
    pub(crate) vm_counts: Arc<tokio::sync::Mutex<HashMap<String, u32>>>,
}

pub(in crate::agent) fn new(config: &Config) -> Result<VmHost, ConsentError> {
    let project_root = std::env::current_dir()
        .map_err(|source| ConsentError::CurrentDirectoryUnavailable { source })?;
    let settings = config
        .vm_settings()
        .map_err(ConsentError::HostConfiguration)?;
    Ok(VmHost {
        backend: Arc::new(Backend::new(&settings)),
        settings,
        installation_id: project_root.to_string_lossy().into_owned(),
        session_epoch: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        vm_counts: Arc::default(),
    })
}

impl VmHost {
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

    async fn create(
        &self,
        id: &VmIdentity,
        requested: RequestedVmConfig,
    ) -> Result<VmRef, vm::VmError> {
        let config = self.backend_config(requested)?;
        let mut counts = self.vm_counts.lock().await;
        if self.backend.get(id).await?.is_some() {
            return Err(vm::VmError::AlreadyExists);
        }
        let count = counts.get(&id.principal).copied().unwrap_or_default();
        self.enforce_vm_limit(count)?;
        let vm = self.backend.create(id, &config).await?;
        counts.insert(id.principal.clone(), count + 1);
        Ok(vm)
    }

    async fn get_or_create(
        &self,
        id: &VmIdentity,
        requested: RequestedVmConfig,
    ) -> Result<VmRef, vm::VmError> {
        let config = self.backend_config(requested)?;
        let mut counts = self.vm_counts.lock().await;
        let exists = self.backend.get(id).await?.is_some();
        let count = counts.get(&id.principal).copied().unwrap_or_default();
        if !exists {
            self.enforce_vm_limit(count)?;
        }
        let vm = self.backend.get_or_create(id, &config).await?;
        if !exists {
            counts.insert(id.principal.clone(), count + 1);
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
        let mut counts = self.vm_counts.lock().await;
        self.backend.destroy(vm).await?;
        let remove = if let Some(count) = counts.get_mut(principal) {
            *count = count.saturating_sub(1);
            *count == 0
        } else {
            false
        };
        if remove {
            counts.remove(principal);
        }
        Ok(())
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
