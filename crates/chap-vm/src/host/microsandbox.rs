use std::{
    collections::HashSet,
    format,
    net::IpAddr,
    path::{Path, PathBuf},
    string::{String, ToString},
    time::Duration,
    vec::Vec,
};

use microsandbox::{
    ExecEvent, MicrosandboxError, NetworkPolicy, Sandbox,
    sandbox::{SandboxBuilder, SandboxHandle},
};

use super::{
    ExecOutcome, MountSpec, ResolvedImage, Subject, VmBackend, VmCommand, VmConfig, VmError,
    VmIdentity, VmRef, VmSettings,
};

const NAME_PREFIX: &str = "chap-";
const INSTALLATION_LABEL: &str = "chap.installation";
const PRINCIPAL_LABEL: &str = "chap.principal";
const PHYSICAL_LABEL: &str = "chap.physical";
const CONFIG_LABEL: &str = "chap.config";
const LIST_PAGE_SIZE: u32 = 100;
const EXEC_KILL_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct MicrosandboxBackend {
    settings: VmSettings,
}

impl MicrosandboxBackend {
    pub fn new(settings: &VmSettings) -> Self {
        Self {
            settings: settings.clone(),
        }
    }

    async fn handle(vm: &VmRef) -> Result<SandboxHandle, VmError> {
        Sandbox::get(&sandbox_name(vm.physical_label()))
            .await
            .map_err(map_sdk_error)
    }

    async fn sandbox(vm: &VmRef) -> Result<Sandbox, VmError> {
        Self::handle(vm)
            .await?
            .connect()
            .await
            .map_err(map_sdk_error)
    }
}

impl VmBackend for MicrosandboxBackend {
    async fn create(&self, id: &VmIdentity, cfg: &VmConfig) -> Result<VmRef, VmError> {
        let physical_label = id.physical_label();
        sandbox_builder(id, cfg)?
            .create()
            .await
            .map_err(map_sdk_error)?;
        Ok(VmRef { physical_label })
    }

    async fn get(&self, id: &VmIdentity) -> Result<Option<VmRef>, VmError> {
        let physical_label = id.physical_label();
        match Sandbox::get(&sandbox_name(&physical_label)).await {
            Ok(_) => Ok(Some(VmRef { physical_label })),
            Err(MicrosandboxError::SandboxNotFound(_)) => Ok(None),
            Err(error) => Err(map_sdk_error(error)),
        }
    }

    async fn get_or_create(&self, id: &VmIdentity, cfg: &VmConfig) -> Result<VmRef, VmError> {
        let physical_label = id.physical_label();
        let sandbox = sandbox_builder(id, cfg)?
            .connect_or_create()
            .await
            .map_err(map_sdk_error)?;
        if sandbox
            .config()
            .spec
            .labels
            .get(CONFIG_LABEL)
            .map(String::as_str)
            != Some(cfg.config_hash.as_str())
        {
            return Err(VmError::ConfigMismatch);
        }
        Ok(VmRef { physical_label })
    }

    async fn exec(&self, vm: &VmRef, command: VmCommand) -> Result<ExecOutcome, VmError> {
        let mut args = command.args.into_iter();
        let program = args
            .next()
            .ok_or_else(|| VmError::Failed("VM command must not be empty".into()))?;
        let args = args.collect::<Vec<_>>();
        let timeout = Duration::from_millis(
            command
                .timeout_ms
                .unwrap_or(self.settings.max_exec_ms)
                .min(self.settings.max_exec_ms),
        );
        let cwd = command.cwd;
        let stdin = command.stdin;
        let sandbox = Self::sandbox(vm).await?;
        let mut handle = sandbox
            .exec_stream_with(program, move |mut options| {
                options = options.args(args).timeout(timeout);
                if let Some(cwd) = cwd {
                    options = options.cwd(cwd);
                }
                if let Some(stdin) = stdin {
                    options = options.stdin_bytes(stdin);
                }
                options
            })
            .await
            .map_err(map_sdk_error)?;

        match tokio::time::timeout(
            timeout,
            collect_exec(&mut handle, self.settings.max_output_bytes),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                let _ = handle.kill().await;
                let _ = tokio::time::timeout(
                    EXEC_KILL_WAIT,
                    collect_exec(&mut handle, self.settings.max_output_bytes),
                )
                .await;
                Err(VmError::TimedOut)
            }
        }
    }

    async fn read_file(&self, vm: &VmRef, path: &str, max_bytes: u64) -> Result<Vec<u8>, VmError> {
        let sandbox = Self::sandbox(vm).await?;
        let mut stream = sandbox
            .fs()
            .read_stream(path)
            .await
            .map_err(map_sdk_error)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.recv().await.map_err(map_sdk_error)? {
            let chunk_len = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
            let current_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            if chunk_len > max_bytes.saturating_sub(current_len) {
                return Err(VmError::Failed(format!(
                    "file exceeds the {max_bytes}-byte read limit"
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn write_file(&self, vm: &VmRef, path: &str, bytes: &[u8]) -> Result<(), VmError> {
        Self::sandbox(vm)
            .await?
            .fs()
            .write(path, bytes)
            .await
            .map_err(map_sdk_error)
    }

    async fn destroy(&self, vm: &VmRef) -> Result<(), VmError> {
        destroy_handle(Self::handle(vm).await?).await
    }

    async fn owner_of(&self, vm: &VmRef) -> Result<Option<Subject>, VmError> {
        let handle = match Self::handle(vm).await {
            Ok(handle) => handle,
            Err(VmError::NoSuchVm) => return Ok(None),
            Err(error) => return Err(error),
        };
        let config = handle.config().map_err(map_sdk_error)?;
        Ok(config
            .spec
            .labels
            .get(PRINCIPAL_LABEL)
            .cloned()
            .map(Subject))
    }

    async fn reap(&self, installation_id: &str, keep: &[&VmIdentity]) -> Result<(), VmError> {
        let keep = keep
            .iter()
            .map(|identity| identity.physical_label())
            .collect::<HashSet<_>>();
        let mut cursor = None;
        let mut handles = Vec::new();

        loop {
            let page_cursor = cursor.take();
            let page = Sandbox::list_with(|list| {
                let list = list
                    .limit(LIST_PAGE_SIZE)
                    .label(INSTALLATION_LABEL, installation_id);
                match page_cursor {
                    Some(cursor) => list.cursor(cursor),
                    None => list,
                }
            })
            .await
            .map_err(map_sdk_error)?;
            handles.extend(page.sandboxes);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }

        for handle in handles {
            let config = handle.config().map_err(map_sdk_error)?;
            let physical = config.spec.labels.get(PHYSICAL_LABEL);
            if physical.is_none_or(|physical| !keep.contains(physical)) {
                destroy_handle(handle).await?;
            }
        }
        Ok(())
    }
}

fn sandbox_builder(id: &VmIdentity, cfg: &VmConfig) -> Result<SandboxBuilder, VmError> {
    let physical_label = id.physical_label();
    let cpus = u8::try_from(cfg.cpus)
        .map_err(|_| VmError::Failed(format!("VM CPU count {} exceeds u8", cfg.cpus)))?;
    let memory = u32::try_from(cfg.memory_mb).map_err(|_| {
        VmError::Failed(format!("VM memory size {} MiB exceeds u32", cfg.memory_mb))
    })?;
    let mounts = canonical_mounts(&cfg.mounts)?;
    let mut builder = Sandbox::builder(sandbox_name(&physical_label))
        .image(image_reference(&cfg.image))
        .cpus(cpus)
        .memory(memory)
        .detached(true)
        .max_duration(cfg.max_duration_ms.div_ceil(1_000))
        .idle_timeout(cfg.idle_timeout_ms.div_ceil(1_000))
        .label(INSTALLATION_LABEL, &id.installation_id)
        .label(PRINCIPAL_LABEL, &id.principal)
        .label(PHYSICAL_LABEL, &physical_label)
        .label(CONFIG_LABEL, &cfg.config_hash);

    for variable in &cfg.env {
        builder = builder.env(&variable.name, &variable.value);
    }
    for mount in mounts {
        builder = builder.volume(mount.guest, |volume| {
            let volume = volume.bind(mount.host);
            if mount.readonly {
                volume.readonly()
            } else {
                volume
            }
        });
    }

    if cfg.egress.is_empty() {
        return Ok(builder.disable_network());
    }

    let mut policy = NetworkPolicy::builder().default_deny();
    for destination in &cfg.egress {
        let addr = destination.addr();
        let prefix_len = destination.prefix_len();
        let port = destination.port();
        policy = policy.egress(move |rule| {
            if let Some(port) = port {
                rule.port(port);
            }
            if prefix_len == host_prefix_len(addr) {
                rule.allow().ip(addr.to_string())
            } else {
                rule.allow().cidr(format!("{addr}/{prefix_len}"))
            }
        });
    }
    let policy = policy
        .build()
        .map_err(|error| VmError::Failed(error.to_string()))?;
    Ok(builder.network(|network| network.policy(policy)))
}

async fn destroy_handle(handle: SandboxHandle) -> Result<(), VmError> {
    handle.stop().await.map_err(map_sdk_error)?;
    handle.remove().await.map_err(map_sdk_error)
}

async fn collect_exec(
    handle: &mut microsandbox::ExecHandle,
    max_output_bytes: u64,
) -> Result<ExecOutcome, VmError> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut captured = 0;
    let mut truncated = false;

    while let Some(event) = handle.recv().await {
        match event {
            ExecEvent::Started { .. } => {}
            ExecEvent::Stdout(bytes) => {
                truncated |= append_capped(&mut stdout, &bytes, &mut captured, max_output_bytes);
            }
            ExecEvent::Stderr(bytes) => {
                truncated |= append_capped(&mut stderr, &bytes, &mut captured, max_output_bytes);
            }
            ExecEvent::Exited { code } => {
                return Ok(ExecOutcome {
                    exit_code: code,
                    stdout,
                    stderr,
                    truncated,
                });
            }
            ExecEvent::Failed(error) => {
                return Err(map_sdk_error(MicrosandboxError::ExecFailed(error)));
            }
            ExecEvent::StdinError(_) => {}
        }
    }

    Err(VmError::Failed(
        "microsandbox exec session ended without an exit event".into(),
    ))
}

fn append_capped(target: &mut Vec<u8>, bytes: &[u8], captured: &mut u64, limit: u64) -> bool {
    let available = limit.saturating_sub(*captured);
    let bytes_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let take = available.min(bytes_len);
    let take = usize::try_from(take).unwrap_or(bytes.len());
    target.extend_from_slice(&bytes[..take]);
    *captured = captured.saturating_add(u64::try_from(take).unwrap_or(u64::MAX));
    take < bytes.len()
}

fn image_reference(image: &ResolvedImage) -> String {
    let mut reference = format!("{}/{}", image.registry, image.repository);
    if let Some(tag) = &image.tag {
        reference.push(':');
        reference.push_str(tag);
    }
    if let Some(digest) = &image.digest {
        reference.push('@');
        reference.push_str(digest);
    }
    reference
}

fn sandbox_name(physical_label: &str) -> String {
    format!("{NAME_PREFIX}{physical_label}")
}

fn host_prefix_len(addr: IpAddr) -> u8 {
    match addr {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CanonicalMount {
    host: PathBuf,
    guest: String,
    readonly: bool,
}

fn canonical_mounts(mounts: &[MountSpec]) -> Result<Vec<CanonicalMount>, VmError> {
    mounts
        .iter()
        .map(|mount| {
            let host = std::fs::canonicalize(Path::new(&mount.host)).map_err(|error| {
                VmError::Failed(format!(
                    "failed to canonicalize bind root `{}`: {error}",
                    mount.host
                ))
            })?;
            Ok(CanonicalMount {
                host,
                guest: mount.guest.clone(),
                readonly: mount.readonly,
            })
        })
        .collect()
}

fn map_sdk_error(error: MicrosandboxError) -> VmError {
    match error {
        MicrosandboxError::SandboxAlreadyExists(_) => VmError::AlreadyExists,
        MicrosandboxError::SandboxNotFound(_) => VmError::NoSuchVm,
        MicrosandboxError::ExecTimeout(_) => VmError::TimedOut,
        error @ (MicrosandboxError::LibkrunfwNotFound(_)
        | MicrosandboxError::BootStart { .. }
        | MicrosandboxError::Unsupported { .. }) => VmError::Unavailable(error.to_string()),
        error => VmError::Failed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(tag: Option<&str>, digest: Option<&str>) -> ResolvedImage {
        ResolvedImage {
            registry: "docker.io".into(),
            repository: "library/alpine".into(),
            tag: tag.map(String::from),
            digest: digest.map(String::from),
        }
    }

    #[test]
    fn sandbox_names_are_valid_and_derive_from_the_physical_label() {
        let physical = "a".repeat(64);
        let name = sandbox_name(&physical);

        assert_eq!(name, format!("chap-{physical}"));
        assert!(microsandbox::validate_sandbox_name(&name).is_ok());
    }

    #[test]
    fn image_references_include_tag_and_digest_selectors() {
        assert_eq!(
            image_reference(&image(Some("3.20"), None)),
            "docker.io/library/alpine:3.20"
        );
        assert_eq!(
            image_reference(&image(None, Some("sha256:abc"))),
            "docker.io/library/alpine@sha256:abc"
        );
        assert_eq!(
            image_reference(&image(Some("3.20"), Some("sha256:abc"))),
            "docker.io/library/alpine:3.20@sha256:abc"
        );
    }

    #[test]
    fn sdk_errors_map_to_stable_vm_error_categories() {
        assert_eq!(
            map_sdk_error(MicrosandboxError::SandboxAlreadyExists("taken".into())),
            VmError::AlreadyExists
        );
        assert_eq!(
            map_sdk_error(MicrosandboxError::SandboxNotFound("missing".into())),
            VmError::NoSuchVm
        );
        assert_eq!(
            map_sdk_error(MicrosandboxError::ExecTimeout(Duration::from_secs(1))),
            VmError::TimedOut
        );
        assert!(matches!(
            map_sdk_error(MicrosandboxError::LibkrunfwNotFound("gone".into())),
            VmError::Unavailable(message) if message.contains("gone")
        ));
        assert!(matches!(
            map_sdk_error(MicrosandboxError::InvalidConfig("bad".into())),
            VmError::Failed(message) if message.contains("bad")
        ));
    }

    #[test]
    fn bind_roots_are_canonicalized_before_building() {
        let directory = tempfile::tempdir().unwrap();
        let requested = directory.path().join(".");
        let mounts = canonical_mounts(&[MountSpec {
            host: requested.to_string_lossy().into_owned(),
            guest: "/mnt/project".into(),
            readonly: true,
        }])
        .unwrap();

        assert_eq!(mounts[0].host, directory.path().canonicalize().unwrap());
        assert_eq!(mounts[0].guest, "/mnt/project");
        assert!(mounts[0].readonly);
    }

    #[test]
    fn output_is_capped_across_stdout_and_stderr() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut captured = 0;

        assert!(!append_capped(&mut stdout, b"abc", &mut captured, 5));
        assert!(append_capped(&mut stderr, b"def", &mut captured, 5));
        assert!(append_capped(&mut stdout, b"g", &mut captured, 5));
        assert_eq!(stdout, b"abc");
        assert_eq!(stderr, b"de");
        assert_eq!(captured, 5);
    }

    #[test]
    fn max_output_bytes_defaults_to_sixty_four_kibibytes() {
        assert_eq!(VmSettings::default().max_output_bytes, 64 * 1024);
    }
}
