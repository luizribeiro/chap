use std::{
    format,
    future::Future,
    net::IpAddr,
    string::{String, ToString},
    time::{Duration, Instant},
    vec::Vec,
};

use microsandbox::{
    ExecEvent, MicrosandboxError, NetworkPolicy, Sandbox, SecretSource as MicrosandboxSecretSource,
    sandbox::{SandboxBuilder, SandboxHandle},
};

use super::{
    Egress, ExecOutcome, ResolvedImage, VmBackend, VmCommand, VmConfig, VmError, VmIdentity,
    VmPrincipal, VmRef, VmSettings, capture::StreamCapture,
};

const NAME_PREFIX: &str = "chap-";
const INSTALLATION_LABEL: &str = "chap.installation";
const EPOCH_LABEL: &str = "chap.epoch";
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

    pub fn for_session(settings: &VmSettings) -> Result<Self, VmError> {
        let state_dir = microsandbox::config::config()
            .map_err(|error| map_sdk_error("VM state directory lookup", error))?
            .home();
        crate::runtime::validate_state_dir(&state_dir)
            .map_err(|error| VmError::Unavailable(error.to_string()))?;
        Ok(Self::new(settings))
    }

    async fn handle(vm: &VmRef) -> Result<SandboxHandle, VmError> {
        Sandbox::get(&sandbox_name(vm.physical_label()))
            .await
            .map_err(|error| map_sdk_error("VM lookup", error))
    }

    async fn sandbox(vm: &VmRef) -> Result<Sandbox, VmError> {
        Self::handle(vm)
            .await?
            .connect()
            .await
            .map_err(|error| map_sdk_error("VM connection", error))
    }
}

impl VmBackend for MicrosandboxBackend {
    async fn create(&self, id: &VmIdentity, cfg: &VmConfig) -> Result<VmRef, VmError> {
        let physical_label = id.physical_label();
        sandbox_builder(id, cfg)?
            .create()
            .await
            .map_err(|error| map_sdk_error("VM creation", error))?;
        Ok(VmRef { physical_label })
    }

    async fn get(&self, id: &VmIdentity) -> Result<Option<VmRef>, VmError> {
        let physical_label = id.physical_label();
        match Sandbox::get(&sandbox_name(&physical_label)).await {
            Ok(_) => Ok(Some(VmRef { physical_label })),
            Err(MicrosandboxError::SandboxNotFound(_)) => Ok(None),
            Err(error) => Err(map_sdk_error("VM lookup", error)),
        }
    }

    async fn get_or_create(&self, id: &VmIdentity, cfg: &VmConfig) -> Result<VmRef, VmError> {
        let physical_label = id.physical_label();
        match Sandbox::get(&sandbox_name(&physical_label)).await {
            Ok(handle) => connect_existing_if_config_matches(handle, cfg).await?,
            Err(MicrosandboxError::SandboxNotFound(_)) => {
                match sandbox_builder(id, cfg)?.create().await {
                    Ok(_) => {}
                    Err(MicrosandboxError::SandboxAlreadyExists(_)) => {
                        let handle = Sandbox::get(&sandbox_name(&physical_label))
                            .await
                            .map_err(|error| map_sdk_error("VM lookup", error))?;
                        connect_existing_if_config_matches(handle, cfg).await?;
                    }
                    Err(error) => return Err(map_sdk_error("VM creation", error)),
                }
            }
            Err(error) => return Err(map_sdk_error("VM lookup", error)),
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
                .unwrap_or(self.settings.calls.exec_timeout_ceiling_ms)
                .min(self.settings.calls.exec_timeout_ceiling_ms),
        );
        let cwd = command.cwd;
        let stdin = command.stdin;
        let sdk_timeout = timeout.saturating_add(EXEC_KILL_WAIT);
        let sandbox = Self::sandbox(vm).await?;
        let started_at = Instant::now();
        let mut handle = sandbox
            .exec_stream_with(program, move |mut options| {
                options = options.args(args).timeout(sdk_timeout);
                if let Some(cwd) = cwd {
                    options = options.cwd(cwd);
                }
                if let Some(stdin) = stdin {
                    options = options.stdin_bytes(stdin);
                }
                options
            })
            .await
            .map_err(|error| map_sdk_error("VM command execution", error))?;
        let mut capture = ExecCapture::new(self.settings.calls.exec_max_output_bytes, started_at);

        match tokio::time::timeout(timeout, collect_exec(&mut handle, &mut capture)).await {
            Ok(result) => result.map(|exit_code| capture.into_outcome(Some(exit_code))),
            Err(_) => {
                let _ = handle.kill().await;
                let _ =
                    tokio::time::timeout(EXEC_KILL_WAIT, collect_exec(&mut handle, &mut capture))
                        .await;
                Ok(capture.into_outcome(None))
            }
        }
    }

    async fn read_file(&self, vm: &VmRef, path: &str, max_bytes: u64) -> Result<Vec<u8>, VmError> {
        let sandbox = Self::sandbox(vm).await?;
        let mut stream = sandbox
            .fs()
            .read_stream(path)
            .await
            .map_err(|error| map_sdk_error("VM file read", error))?;
        let mut bytes = Vec::new();
        while let Some(chunk) = stream
            .recv()
            .await
            .map_err(|error| map_sdk_error("VM file read", error))?
        {
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
            .map_err(|error| map_sdk_error("VM file write", error))
    }

    async fn destroy(&self, vm: &VmRef) -> Result<(), VmError> {
        destroy_handle(Self::handle(vm).await?).await
    }

    async fn owner_of(&self, vm: &VmRef) -> Result<Option<VmPrincipal>, VmError> {
        let handle = match Self::handle(vm).await {
            Ok(handle) => handle,
            Err(VmError::NoSuchVm) => return Ok(None),
            Err(error) => return Err(error),
        };
        let config = handle
            .config()
            .map_err(|error| map_sdk_error("VM metadata read", error))?;
        config
            .spec
            .labels
            .get(PRINCIPAL_LABEL)
            .map(String::as_str)
            .map(VmPrincipal::from_label)
            .transpose()
    }

    async fn reap(&self, installation_id: &str, current_epoch: u64) -> Result<(), VmError> {
        destroy_installation_vms(installation_id, |epoch| epoch != Some(current_epoch)).await
    }

    async fn shutdown(&self, installation_id: &str, session_epoch: u64) -> Result<(), VmError> {
        destroy_installation_vms(installation_id, |epoch| epoch == Some(session_epoch)).await
    }
}

async fn connect_existing_if_config_matches(
    handle: SandboxHandle,
    cfg: &VmConfig,
) -> Result<(), VmError> {
    let config = handle
        .config()
        .map_err(|error| map_sdk_error("VM metadata read", error))?;
    connect_if_config_matches(
        config.spec.labels.get(CONFIG_LABEL).map(String::as_str),
        &cfg.config_hash,
        async {
            handle
                .connect_or_start_detached()
                .await
                .map_err(|error| map_sdk_error("VM connection", error))
        },
    )
    .await?;
    Ok(())
}

async fn connect_if_config_matches<T>(
    actual: Option<&str>,
    expected: &str,
    connect: impl Future<Output = Result<T, VmError>>,
) -> Result<T, VmError> {
    if actual != Some(expected) {
        return Err(VmError::ConfigMismatch);
    }
    connect.await
}

async fn destroy_installation_vms(
    installation_id: &str,
    should_destroy: impl Fn(Option<u64>) -> bool,
) -> Result<(), VmError> {
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
        .map_err(|error| map_sdk_error("VM listing", error))?;
        handles.extend(page.sandboxes);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    for handle in handles {
        let config = handle
            .config()
            .map_err(|error| map_sdk_error("VM metadata read", error))?;
        let epoch = config
            .spec
            .labels
            .get(EPOCH_LABEL)
            .and_then(|epoch| epoch.parse().ok());
        if should_destroy(epoch) {
            destroy_handle(handle).await?;
        }
    }
    Ok(())
}

fn sandbox_builder(id: &VmIdentity, cfg: &VmConfig) -> Result<SandboxBuilder, VmError> {
    let physical_label = id.physical_label();
    let cpus = u8::try_from(cfg.cpus)
        .map_err(|_| VmError::Failed(format!("VM CPU count {} exceeds u8", cfg.cpus)))?;
    let memory = u32::try_from(cfg.memory_mb).map_err(|_| {
        VmError::Failed(format!("VM memory size {} MiB exceeds u32", cfg.memory_mb))
    })?;
    let mut builder = Sandbox::builder(sandbox_name(&physical_label))
        .image(image_reference(&cfg.image))
        .cpus(cpus)
        .memory(memory)
        .detached(true)
        .max_duration(cfg.max_lifetime_ms.div_ceil(1_000))
        .idle_timeout(cfg.idle_timeout_ms.div_ceil(1_000))
        .label(INSTALLATION_LABEL, &id.installation_id)
        .label(EPOCH_LABEL, id.session_epoch.to_string())
        .label(PRINCIPAL_LABEL, id.principal.label())
        .label(PHYSICAL_LABEL, &physical_label)
        .label(CONFIG_LABEL, &cfg.config_hash);

    for variable in &cfg.env {
        builder = builder.env(&variable.name, &variable.value);
    }
    for mount in &cfg.mounts {
        builder = builder.volume(mount.guest.clone(), |volume| {
            let volume = volume.bind(&mount.host);
            if mount.readonly {
                volume.readonly()
            } else {
                volume
            }
        });
    }

    builder = if cfg.egress.is_empty() {
        builder.disable_network()
    } else {
        let policy = network_policy(&cfg.egress)?;
        builder.network(|network| network.policy(policy))
    };

    for secret in &cfg.secrets {
        let super::SecretSource::HostEnv(variable) = &secret.source;
        builder = builder.secret(|builder| {
            let mut builder = builder
                .env(&secret.env)
                .source(MicrosandboxSecretSource::Env {
                    var: variable.clone(),
                });
            for host in &secret.hosts {
                builder = builder.allow(host);
            }
            builder
        });
    }

    Ok(builder)
}

// FIXME(#43): address rules admit every service behind a shared address, and the
// whole-family port-53 grant reaches every DNS server, so egress grants are broader than
// they read. Name-based scopes on microsandbox's `domain` rules replace both.
fn network_policy(egress: &[Egress]) -> Result<NetworkPolicy, VmError> {
    let mut policy = NetworkPolicy::builder().default_deny();
    if egress.iter().any(enables_gateway_dns) {
        // A whole-family port-53 grant covers the gateway resolver, whose DNS query has no
        // destination IP for the CIDR rule to match.
        policy = policy.egress(|rule| rule.udp().tcp().port(53).allow_host());
    }
    for destination in egress {
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
    let policy = policy.build().map_err(|error| {
        tracing::warn!(operation = "VM network policy", error = %error, "microsandbox operation failed");
        VmError::Failed("VM network policy configuration failed".into())
    })?;
    Ok(policy)
}

fn enables_gateway_dns(destination: &Egress) -> bool {
    destination.port().is_none_or(|port| port == 53)
        && destination.prefix_len() == 0
        && destination.addr().is_unspecified()
}

async fn destroy_handle(handle: SandboxHandle) -> Result<(), VmError> {
    handle
        .stop()
        .await
        .map_err(|error| map_sdk_error("VM stop", error))?;
    handle
        .remove()
        .await
        .map_err(|error| map_sdk_error("VM removal", error))
}

async fn collect_exec(
    handle: &mut microsandbox::ExecHandle,
    capture: &mut ExecCapture,
) -> Result<i32, VmError> {
    while let Some(event) = handle.recv().await {
        match event {
            ExecEvent::Started { .. } => {}
            ExecEvent::Stdout(bytes) => {
                capture.stdout.append(&bytes);
            }
            ExecEvent::Stderr(bytes) => {
                capture.stderr.append(&bytes);
            }
            ExecEvent::Exited { code } => return Ok(code),
            ExecEvent::Failed(error) => {
                return Err(map_sdk_error(
                    "VM command execution",
                    MicrosandboxError::ExecFailed(error),
                ));
            }
            ExecEvent::StdinError(_) => {}
        }
    }

    Err(VmError::Failed(
        "microsandbox exec session ended without an exit event".into(),
    ))
}

struct ExecCapture {
    started_at: Instant,
    stdout: StreamCapture,
    stderr: StreamCapture,
}

impl ExecCapture {
    fn new(limit: u64, started_at: Instant) -> Self {
        Self {
            started_at,
            stdout: StreamCapture::new(limit),
            stderr: StreamCapture::new(limit),
        }
    }

    fn into_outcome(self, exit_code: Option<i32>) -> ExecOutcome {
        let truncated = self.stdout.truncated() || self.stderr.truncated();
        ExecOutcome {
            exit_code,
            wall_time_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout: self.stdout.into_bytes(),
            stderr: self.stderr.into_bytes(),
            truncated,
        }
    }
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

fn map_sdk_error(operation: &'static str, error: MicrosandboxError) -> VmError {
    tracing::warn!(operation, error = %error, "microsandbox operation failed");
    match error {
        MicrosandboxError::SandboxAlreadyExists(_) => VmError::AlreadyExists,
        MicrosandboxError::SandboxNotFound(_) => VmError::NoSuchVm,
        MicrosandboxError::ExecTimeout(_) => VmError::TimedOut,
        MicrosandboxError::LibkrunfwNotFound(_)
        | MicrosandboxError::RuntimeNotInstalled(_)
        | MicrosandboxError::RuntimeIncomplete(_)
        | MicrosandboxError::BootStart { .. }
        | MicrosandboxError::Unsupported { .. } => {
            VmError::Unavailable(format!("{operation} is unavailable"))
        }
        _ => VmError::Failed(format!("{operation} failed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn image(tag: Option<&str>, digest: Option<&str>) -> ResolvedImage {
        ResolvedImage {
            registry: "docker.io".into(),
            repository: "library/alpine".into(),
            tag: tag.map(String::from),
            digest: digest.map(String::from),
        }
    }

    #[test]
    fn a_session_checks_the_state_directory_the_sdk_resolved() {
        let too_long = std::env::temp_dir().join("x".repeat(64));
        // SAFETY: no other test in this crate reads or writes MSB_HOME.
        unsafe { std::env::set_var("MSB_HOME", &too_long) };
        let error = MicrosandboxBackend::for_session(&VmSettings::default())
            .err()
            .expect("a state directory too long for socket paths is rejected");
        assert!(matches!(
            &error,
            VmError::Unavailable(message)
                if message.contains(too_long.to_str().unwrap()) && message.contains("51-byte limit")
        ));

        unsafe { std::env::set_var("MSB_HOME", "/tmp/chap-vm-state") };
        assert!(MicrosandboxBackend::for_session(&VmSettings::default()).is_ok());
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
            map_sdk_error(
                "VM creation",
                MicrosandboxError::SandboxAlreadyExists("taken".into())
            ),
            VmError::AlreadyExists
        );
        assert_eq!(
            map_sdk_error(
                "VM lookup",
                MicrosandboxError::SandboxNotFound("missing".into())
            ),
            VmError::NoSuchVm
        );
        assert_eq!(
            map_sdk_error(
                "VM command execution",
                MicrosandboxError::ExecTimeout(Duration::from_secs(1))
            ),
            VmError::TimedOut
        );
        assert!(matches!(
            map_sdk_error(
                "VM creation",
                MicrosandboxError::LibkrunfwNotFound(
                    "/Users/alice/.microsandbox/lib/libkrunfw.dylib".into()
                )
            ),
            VmError::Unavailable(message) if message == "VM creation is unavailable"
        ));
        assert!(matches!(
            map_sdk_error(
                "VM creation",
                MicrosandboxError::RuntimeNotInstalled("runtime pair missing".into())
            ),
            VmError::Unavailable(message) if message == "VM creation is unavailable"
        ));
        assert!(matches!(
            map_sdk_error(
                "VM creation",
                MicrosandboxError::RuntimeIncomplete("libkrunfw missing next to msb".into())
            ),
            VmError::Unavailable(message) if message == "VM creation is unavailable"
        ));
        assert!(matches!(
            map_sdk_error(
                "VM creation",
                MicrosandboxError::InvalidConfig(
                    "invalid path /Users/alice/.microsandbox/state".into()
                )
            ),
            VmError::Failed(message) if message == "VM creation failed"
        ));
    }

    #[tokio::test]
    async fn a_config_mismatch_is_rejected_before_connecting_or_restarting() {
        let connected = AtomicBool::new(false);

        let result = connect_if_config_matches(Some("old"), "new", async {
            connected.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await;

        assert_eq!(result, Err(VmError::ConfigMismatch));
        assert!(!connected.load(Ordering::SeqCst));
    }

    #[test]
    fn output_is_capped_independently_per_stream() {
        let mut capture = ExecCapture::new(6, Instant::now());
        capture.stdout.append(b"01\n02\n03\n04\n");
        capture.stderr.append(b"E1\nE2\n");

        let outcome = capture.into_outcome(Some(7));

        assert_eq!(outcome.stdout, b"01\n\n[... 6 bytes omitted ...]\n04\n");
        assert_eq!(outcome.stderr, b"E1\nE2\n");
        assert!(outcome.truncated);
    }

    #[test]
    fn exec_output_limit_defaults_to_sixty_four_kibibytes() {
        assert_eq!(VmSettings::default().calls.exec_max_output_bytes, 64 * 1024);
    }

    fn policy(scopes: &[&str]) -> Value {
        let egress = scopes
            .iter()
            .map(|scope| scope.parse().unwrap())
            .collect::<Vec<_>>();
        serde_json::to_value(network_policy(&egress).unwrap()).unwrap()
    }

    fn dns_rules(policy: &Value) -> Vec<&Value> {
        policy["rules"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|rule| rule["destination"] == json!({ "group": "host" }))
            .collect()
    }

    #[test]
    fn whole_ipv4_port_53_grant_adds_gateway_dns_and_cidr_rules() {
        let policy = policy(&["0.0.0.0/0:53"]);
        let rules = policy["rules"].as_array().unwrap();

        assert_eq!(rules.len(), 2);
        assert_eq!(dns_rules(&policy).len(), 1);
        assert_eq!(rules[0]["protocols"], json!(["udp", "tcp"]));
        assert_eq!(rules[0]["ports"], json!([{ "start": 53, "end": 53 }]));
        assert_eq!(rules[1]["destination"], json!({ "cidr": "0.0.0.0/0" }));
    }

    #[test]
    fn whole_ipv4_any_port_grant_adds_gateway_dns_and_cidr_rules() {
        let policy = policy(&["0.0.0.0/0:*"]);
        let rules = policy["rules"].as_array().unwrap();

        assert_eq!(rules.len(), 2);
        assert_eq!(dns_rules(&policy).len(), 1);
        assert_eq!(rules[0]["protocols"], json!(["udp", "tcp"]));
        assert_eq!(rules[0]["ports"], json!([{ "start": 53, "end": 53 }]));
        assert_eq!(rules[1]["destination"], json!({ "cidr": "0.0.0.0/0" }));
    }

    #[test]
    fn whole_family_non_dns_grant_does_not_add_gateway_dns() {
        let policy = policy(&["0.0.0.0/0:443"]);

        assert!(dns_rules(&policy).is_empty());
        assert_eq!(policy["rules"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn narrow_port_53_grant_does_not_add_gateway_dns() {
        let policy = policy(&["10.0.0.0/8:53"]);

        assert!(dns_rules(&policy).is_empty());
        assert_eq!(policy["rules"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn whole_ipv6_port_53_grant_adds_gateway_dns_and_cidr_rules() {
        let policy = policy(&["[::]/0:53"]);
        let rules = policy["rules"].as_array().unwrap();

        assert_eq!(rules.len(), 2);
        assert_eq!(dns_rules(&policy).len(), 1);
        assert_eq!(rules[1]["destination"], json!({ "cidr": "::/0" }));
    }
}
