use std::{
    collections::HashMap,
    format,
    string::String,
    sync::{Mutex, MutexGuard, OnceLock, PoisonError},
    vec::Vec,
};

use super::{
    ExecOutcome, SecretSpec, VmBackend, VmCommand, VmConfig, VmError, VmIdentity, VmPrincipal,
    VmRef,
};

#[derive(Default)]
pub struct MockVmBackend {
    vms: Mutex<HashMap<String, MockVm>>,
}

struct MockVm {
    owner: VmPrincipal,
    config_hash: String,
    installation_id: String,
    session_epoch: u64,
    secrets: Vec<SecretSpec>,
    files: HashMap<String, Vec<u8>>,
}

impl MockVmBackend {
    pub fn new(_settings: &super::VmSettings) -> Self {
        Self::default()
    }

    pub fn for_session(settings: &super::VmSettings) -> Result<Self, VmError> {
        Ok(Self::new(settings))
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, MockVm>> {
        self.vms.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn secrets(&self, vm: &VmRef) -> Option<Vec<SecretSpec>> {
        self.lock()
            .get(vm.physical_label())
            .map(|vm| vm.secrets.clone())
    }

    pub fn recorded_secrets() -> Vec<SecretSpec> {
        recorded_secrets()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn recorded_commands() -> Vec<VmCommand> {
        recorded_commands()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn record_host_secrets(id: &VmIdentity, cfg: &VmConfig) {
        if id.principal == VmPrincipal::Host {
            recorded_secrets()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(cfg.secrets.clone());
        }
    }
}

fn recorded_secrets() -> &'static Mutex<Vec<SecretSpec>> {
    static SECRETS: OnceLock<Mutex<Vec<SecretSpec>>> = OnceLock::new();
    SECRETS.get_or_init(Mutex::default)
}

fn recorded_commands() -> &'static Mutex<Vec<VmCommand>> {
    static COMMANDS: OnceLock<Mutex<Vec<VmCommand>>> = OnceLock::new();
    COMMANDS.get_or_init(Mutex::default)
}

impl MockVm {
    fn new(id: &VmIdentity, cfg: &VmConfig) -> Self {
        Self {
            owner: id.principal.clone(),
            config_hash: cfg.config_hash.clone(),
            installation_id: id.installation_id.clone(),
            session_epoch: id.session_epoch,
            secrets: cfg.secrets.clone(),
            files: HashMap::new(),
        }
    }
}

impl VmBackend for MockVmBackend {
    async fn create(&self, id: &VmIdentity, cfg: &VmConfig) -> Result<VmRef, VmError> {
        let physical_label = id.physical_label();
        let mut vms = self.lock();
        if vms.contains_key(&physical_label) {
            return Err(VmError::AlreadyExists);
        }
        Self::record_host_secrets(id, cfg);
        vms.insert(physical_label.clone(), MockVm::new(id, cfg));
        Ok(VmRef { physical_label })
    }

    async fn get(&self, id: &VmIdentity) -> Result<Option<VmRef>, VmError> {
        let physical_label = id.physical_label();
        Ok(self
            .lock()
            .contains_key(&physical_label)
            .then_some(VmRef { physical_label }))
    }

    async fn get_or_create(&self, id: &VmIdentity, cfg: &VmConfig) -> Result<VmRef, VmError> {
        let physical_label = id.physical_label();
        let mut vms = self.lock();
        if let Some(existing) = vms.get(&physical_label) {
            if existing.config_hash != cfg.config_hash {
                return Err(VmError::ConfigMismatch);
            }
            return Ok(VmRef { physical_label });
        }
        Self::record_host_secrets(id, cfg);
        vms.insert(physical_label.clone(), MockVm::new(id, cfg));
        Ok(VmRef { physical_label })
    }

    async fn exec(&self, vm: &VmRef, command: VmCommand) -> Result<ExecOutcome, VmError> {
        {
            let vms = self.lock();
            let vm = vms.get(vm.physical_label()).ok_or(VmError::NoSuchVm)?;
            if vm.owner == VmPrincipal::Host {
                recorded_commands()
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(command.clone());
            }
        }
        let timed_out = command.args.iter().any(|arg| arg.contains("mock-timeout"));
        Ok(ExecOutcome {
            exit_code: (!timed_out).then_some(0),
            stdout: command.args.join(" ").into_bytes(),
            stderr: Vec::new(),
            truncated: false,
        })
    }

    async fn read_file(&self, vm: &VmRef, path: &str, max_bytes: u64) -> Result<Vec<u8>, VmError> {
        let vms = self.lock();
        let vm = vms.get(vm.physical_label()).ok_or(VmError::NoSuchVm)?;
        let bytes = vm
            .files
            .get(path)
            .ok_or_else(|| VmError::Failed(String::from("no such file")))?;
        if u64::try_from(bytes.len()).map_or(true, |len| len > max_bytes) {
            return Err(VmError::Failed(format!(
                "file exceeds the {max_bytes}-byte read limit"
            )));
        }
        Ok(bytes.clone())
    }

    async fn write_file(&self, vm: &VmRef, path: &str, bytes: &[u8]) -> Result<(), VmError> {
        let mut vms = self.lock();
        let vm = vms.get_mut(vm.physical_label()).ok_or(VmError::NoSuchVm)?;
        vm.files.insert(path.into(), bytes.into());
        Ok(())
    }

    async fn destroy(&self, vm: &VmRef) -> Result<(), VmError> {
        if self.lock().remove(vm.physical_label()).is_some() {
            Ok(())
        } else {
            Err(VmError::NoSuchVm)
        }
    }

    async fn owner_of(&self, vm: &VmRef) -> Result<Option<VmPrincipal>, VmError> {
        Ok(self
            .lock()
            .get(vm.physical_label())
            .map(|vm| vm.owner.clone()))
    }

    async fn reap(&self, installation_id: &str, current_epoch: u64) -> Result<(), VmError> {
        self.lock().retain(|_, vm| {
            vm.installation_id != installation_id || vm.session_epoch == current_epoch
        });
        Ok(())
    }

    async fn shutdown(&self, installation_id: &str, session_epoch: u64) -> Result<(), VmError> {
        self.lock().retain(|_, vm| {
            vm.installation_id != installation_id || vm.session_epoch != session_epoch
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        boxed::Box,
        future::Future,
        string::String,
        task::{Context, Poll, Waker},
        thread, vec,
    };

    use super::MockVmBackend;
    use crate::host::{
        ResolvedImage, SecretSource, SecretSpec, VmBackend, VmCommand, VmConfig, VmError,
        VmIdentity, VmPrincipal, VmSettings,
    };

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => thread::yield_now(),
            }
        }
    }

    fn identity(installation_id: &str, epoch: u64, plugin_id: &str, name: &str) -> VmIdentity {
        VmIdentity {
            installation_id: installation_id.into(),
            session_epoch: epoch,
            principal: VmPrincipal::Plugin(plugin_id.parse().unwrap()),
            logical_name: name.into(),
        }
    }

    fn config(hash: &str) -> VmConfig {
        VmConfig {
            image: ResolvedImage {
                registry: "ghcr.io".into(),
                repository: "acme/build".into(),
                tag: Some("1.2".into()),
                digest: None,
            },
            mounts: vec![],
            egress: vec![],
            env: vec![],
            secrets: vec![],
            cpus: 1,
            memory_mb: 512,
            max_lifetime_ms: 3_600_000,
            idle_timeout_ms: 300_000,
            config_hash: hash.into(),
        }
    }

    fn command(args: &[&str], timeout_ms: Option<u64>) -> VmCommand {
        VmCommand {
            args: args.iter().map(|arg| String::from(*arg)).collect(),
            cwd: Some("/work".into()),
            stdin: None,
            timeout_ms,
        }
    }

    #[test]
    fn create_then_get_returns_the_same_vm() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let id = identity("installation-a", 7, "builder-grant-a", "build-env");
            let created = backend.create(&id, &config("hash-a")).await.unwrap();

            assert_eq!(created.physical_label(), id.physical_label());
            assert_eq!(backend.get(&id).await.unwrap(), Some(created));
        });
    }

    #[test]
    fn create_records_secret_specs() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let id = identity("installation-a", 7, "builder-grant-a", "build-env");
            let mut config = config("hash-a");
            config.secrets = vec![SecretSpec {
                env: "GITHUB_TOKEN".into(),
                source: SecretSource::HostEnv("CHAP_GITHUB_TOKEN".into()),
                hosts: vec!["api.github.com".into(), "github.com".into()],
            }];

            let vm = backend.create(&id, &config).await.unwrap();

            assert_eq!(backend.secrets(&vm), Some(config.secrets));
        });
    }

    #[test]
    fn create_rejects_an_existing_identity() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let id = identity("installation-a", 7, "builder-grant-a", "build-env");
            backend.create(&id, &config("hash-a")).await.unwrap();

            assert_eq!(
                backend.create(&id, &config("hash-a")).await,
                Err(VmError::AlreadyExists)
            );
        });
    }

    #[test]
    fn get_or_create_reuses_only_a_matching_config() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let id = identity("installation-a", 7, "builder-grant-a", "build-env");
            let created = backend.get_or_create(&id, &config("hash-a")).await.unwrap();

            assert_eq!(
                backend.get_or_create(&id, &config("hash-a")).await.unwrap(),
                created
            );
            assert_eq!(
                backend.get_or_create(&id, &config("hash-b")).await,
                Err(VmError::ConfigMismatch)
            );
        });
    }

    #[test]
    fn guest_files_round_trip_and_missing_files_are_distinct() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let id = identity("installation-a", 7, "builder-grant-a", "build-env");
            let vm = backend.create(&id, &config("hash-a")).await.unwrap();

            backend
                .write_file(&vm, "/work/result.wasm", b"wasm-bytes")
                .await
                .unwrap();
            assert_eq!(
                backend
                    .read_file(&vm, "/work/result.wasm", 1024)
                    .await
                    .unwrap(),
                b"wasm-bytes"
            );
            assert_eq!(
                backend.read_file(&vm, "/work/missing", 1024).await,
                Err(VmError::Failed(String::from("no such file")))
            );
        });
    }

    #[test]
    fn guest_file_reads_enforce_the_byte_limit() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let id = identity("installation-a", 7, "builder-grant-a", "build-env");
            let vm = backend.create(&id, &config("hash-a")).await.unwrap();
            backend
                .write_file(&vm, "/work/result.wasm", b"wasm-bytes")
                .await
                .unwrap();

            assert!(matches!(
                backend.read_file(&vm, "/work/result.wasm", 4).await,
                Err(VmError::Failed(_))
            ));
            assert_eq!(
                backend
                    .read_file(&vm, "/work/result.wasm", 10)
                    .await
                    .unwrap(),
                b"wasm-bytes"
            );
        });
    }

    #[test]
    fn exec_returns_deterministic_output_and_partial_output_on_timeout() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let id = identity("installation-a", 7, "builder-grant-a", "build-env");
            let vm = backend.create(&id, &config("hash-a")).await.unwrap();

            let outcome = backend
                .exec(&vm, command(&["nix", "build", ".#plugin"], Some(30_000)))
                .await
                .unwrap();
            assert_eq!(outcome.exit_code, Some(0));
            assert_eq!(outcome.stdout, b"nix build .#plugin");
            assert!(outcome.stderr.is_empty());
            assert!(!outcome.truncated);
            let timed_out = backend
                .exec(&vm, command(&["nix", "build", "mock-timeout"], Some(7_000)))
                .await
                .unwrap();
            assert_eq!(timed_out.exit_code, None);
            assert_eq!(timed_out.stdout, b"nix build mock-timeout");
            assert!(timed_out.stderr.is_empty());
            assert!(!timed_out.truncated);
        });
    }

    #[test]
    fn destroy_removes_the_vm() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let id = identity("installation-a", 7, "builder-grant-a", "build-env");
            let vm = backend.create(&id, &config("hash-a")).await.unwrap();

            backend.destroy(&vm).await.unwrap();
            assert_eq!(backend.get(&id).await.unwrap(), None);
            assert_eq!(backend.destroy(&vm).await, Err(VmError::NoSuchVm));
        });
    }

    #[test]
    fn owner_is_the_creating_principal() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let id = identity("installation-a", 7, "builder-grant-a", "build-env");
            let vm = backend.create(&id, &config("hash-a")).await.unwrap();

            assert_eq!(
                backend.owner_of(&vm).await.unwrap(),
                Some(VmPrincipal::Plugin("builder-grant-a".parse().unwrap()))
            );
        });
    }

    #[test]
    fn equal_logical_names_under_different_principals_are_isolated() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let first = identity("installation-a", 7, "builder-grant-a", "build-env");
            let second = identity("installation-a", 7, "builder-grant-b", "build-env");
            let host = VmIdentity {
                principal: VmPrincipal::Host,
                ..first.clone()
            };
            let first_vm = backend.create(&first, &config("hash-a")).await.unwrap();
            let second_vm = backend.create(&second, &config("hash-a")).await.unwrap();
            let host_vm = backend.create(&host, &config("hash-a")).await.unwrap();

            assert_ne!(first_vm, second_vm);
            assert_ne!(first_vm, host_vm);
            assert_ne!(second_vm, host_vm);
            assert_eq!(
                backend.owner_of(&host_vm).await.unwrap(),
                Some(VmPrincipal::Host)
            );
            backend
                .write_file(&first_vm, "/work/owner", b"grant-a")
                .await
                .unwrap();
            assert_eq!(
                backend.read_file(&second_vm, "/work/owner", 1024).await,
                Err(VmError::Failed(String::from("no such file")))
            );
        });
    }

    #[test]
    fn startup_reap_is_scoped_to_installation_and_preserves_the_current_epoch() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let keep = identity("installation-a", 8, "builder-grant-a", "keep");
            let same_epoch = identity("installation-a", 8, "builder-grant-a", "same-epoch");
            let prior_epoch = identity("installation-a", 7, "builder-grant-a", "stale");
            let other_installation = identity("installation-b", 7, "builder-grant-a", "other");
            for id in [&keep, &same_epoch, &prior_epoch, &other_installation] {
                backend.create(id, &config("hash-a")).await.unwrap();
            }

            backend
                .reap("installation-a", keep.session_epoch)
                .await
                .unwrap();

            assert!(backend.get(&keep).await.unwrap().is_some());
            assert!(backend.get(&same_epoch).await.unwrap().is_some());
            assert_eq!(backend.get(&prior_epoch).await.unwrap(), None);
            assert!(backend.get(&other_installation).await.unwrap().is_some());
        });
    }

    #[test]
    fn shutdown_destroys_only_the_current_epochs_vms() {
        block_on(async {
            let backend = MockVmBackend::new(&VmSettings::default());
            let first = identity("installation-a", 8, "builder-grant-a", "first");
            let second = identity("installation-a", 8, "builder-grant-b", "second");
            let prior_epoch = identity("installation-a", 7, "builder-grant-a", "prior");
            let other_installation = identity("installation-b", 8, "builder-grant-a", "other");
            for id in [&first, &second, &prior_epoch, &other_installation] {
                backend.create(id, &config("hash-a")).await.unwrap();
            }

            backend
                .shutdown("installation-a", first.session_epoch)
                .await
                .unwrap();

            assert_eq!(backend.get(&first).await.unwrap(), None);
            assert_eq!(backend.get(&second).await.unwrap(), None);
            assert!(backend.get(&prior_epoch).await.unwrap().is_some());
            assert!(backend.get(&other_installation).await.unwrap().is_some());
        });
    }
}
