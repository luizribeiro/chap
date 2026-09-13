use core::fmt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    format,
    string::{String, ToString},
    vec::Vec,
};

use lockgate_policy::{PluginId, ScopeRepr};

use crate::vm::{Egress, normalize_absolute_path};

#[cfg(feature = "mock")]
mod mock;
#[cfg(feature = "mock")]
pub use mock::MockVmBackend as Backend;
#[cfg(feature = "microsandbox")]
mod microsandbox;
#[cfg(feature = "microsandbox")]
pub use microsandbox::MicrosandboxBackend as Backend;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmIdentity {
    pub installation_id: String,
    pub session_epoch: u64,
    pub plugin_id: PluginId,
    pub logical_name: String,
}

impl VmIdentity {
    /// Returns hex SHA-256 over length-delimited identity components.
    pub fn physical_label(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"chap-vm-identity-v0");
        hash_component(&mut hash, self.installation_id.as_bytes());
        hash_component(&mut hash, &self.session_epoch.to_be_bytes());
        hash_component(&mut hash, self.plugin_id.as_str().as_bytes());
        hash_component(&mut hash, self.logical_name.as_bytes());
        hex_digest(hash.finalize())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountSpec {
    pub host: String,
    pub guest: String,
    pub readonly: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestedVmConfig {
    pub image: OciReference,
    pub mounts: Vec<MountSpec>,
    pub egress: Vec<Egress>,
    pub env: Vec<EnvVar>,
}

impl RequestedVmConfig {
    pub fn normalized(
        image: OciReference,
        mut mounts: Vec<MountSpec>,
        mut egress: Vec<Egress>,
        mut env: Vec<EnvVar>,
    ) -> Result<Self, VmError> {
        for mount in &mut mounts {
            mount.host = normalize_absolute_path(&mount.host)
                .map_err(|error| VmError::Failed(error.to_string()))?;
            mount.guest = normalize_absolute_path(&mount.guest)
                .map_err(|error| VmError::Failed(error.to_string()))?;
        }
        mounts.sort_by(|left, right| {
            (&left.guest, &left.host, left.readonly).cmp(&(
                &right.guest,
                &right.host,
                right.readonly,
            ))
        });
        let mut normalized_mounts: Vec<MountSpec> = Vec::with_capacity(mounts.len());
        for mount in mounts {
            if let Some(previous) = normalized_mounts.last() {
                if previous == &mount {
                    continue;
                }
                if previous.guest == mount.guest {
                    return Err(VmError::Failed(format!(
                        "conflicting mounts for guest path `{}`",
                        mount.guest
                    )));
                }
            }
            normalized_mounts.push(mount);
        }

        egress.sort_by_key(ScopeRepr::canonical);
        egress.dedup();

        env.sort_by(|left, right| (&left.name, &left.value).cmp(&(&right.name, &right.value)));
        let mut normalized_env: Vec<EnvVar> = Vec::with_capacity(env.len());
        for variable in env {
            if let Some(previous) = normalized_env.last() {
                if previous == &variable {
                    continue;
                }
                if previous.name == variable.name {
                    return Err(VmError::Failed(format!(
                        "conflicting values for environment variable `{}`",
                        variable.name
                    )));
                }
            }
            normalized_env.push(variable);
        }

        Ok(Self {
            image,
            mounts: normalized_mounts,
            egress,
            env: normalized_env,
        })
    }

    /// Returns hex SHA-256 over the canonical requested configuration.
    pub fn canonical_hash(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"chap-vm-requested-config-v0");
        hash_component(&mut hash, self.image.canonical().as_bytes());
        hash_len(&mut hash, self.mounts.len());
        for mount in &self.mounts {
            hash_component(&mut hash, mount.host.as_bytes());
            hash_component(&mut hash, mount.guest.as_bytes());
            hash_component(&mut hash, &[u8::from(mount.readonly)]);
        }
        hash_len(&mut hash, self.egress.len());
        for destination in &self.egress {
            hash_component(&mut hash, destination.canonical().as_bytes());
        }
        hash_len(&mut hash, self.env.len());
        for variable in &self.env {
            hash_component(&mut hash, variable.name.as_bytes());
            hash_component(&mut hash, variable.value.as_bytes());
        }
        hex_digest(hash.finalize())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmConfig {
    pub image: ResolvedImage,
    pub mounts: Vec<MountSpec>,
    pub egress: Vec<Egress>,
    pub env: Vec<EnvVar>,
    pub cpus: u32,
    pub memory_mb: u64,
    pub max_lifetime_ms: u64,
    pub idle_timeout_ms: u64,
    pub config_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmCommand {
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub stdin: Option<Vec<u8>>,
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecOutcome {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmRef {
    physical_label: String,
}

impl VmRef {
    pub fn physical_label(&self) -> &str {
        &self.physical_label
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmError {
    AlreadyExists,
    NoSuchVm,
    ConfigMismatch,
    TimedOut,
    Denied(String),
    Failed(String),
    Unavailable(String),
}

impl fmt::Display for VmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyExists => formatter.write_str("VM already exists"),
            Self::NoSuchVm => formatter.write_str("no such VM"),
            Self::ConfigMismatch => formatter.write_str("VM configuration does not match"),
            Self::TimedOut => formatter.write_str("VM operation timed out"),
            Self::Denied(message) | Self::Failed(message) | Self::Unavailable(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for VmError {}

#[allow(
    async_fn_in_trait,
    reason = "VM backends are selected statically and are never used through dyn"
)]
pub trait VmBackend: Send + Sync + 'static {
    async fn create(&self, id: &VmIdentity, cfg: &VmConfig) -> Result<VmRef, VmError>;
    async fn get(&self, id: &VmIdentity) -> Result<Option<VmRef>, VmError>;
    async fn get_or_create(&self, id: &VmIdentity, cfg: &VmConfig) -> Result<VmRef, VmError>;
    async fn exec(&self, vm: &VmRef, command: VmCommand) -> Result<ExecOutcome, VmError>;
    async fn read_file(&self, vm: &VmRef, path: &str, max_bytes: u64) -> Result<Vec<u8>, VmError>;
    async fn write_file(&self, vm: &VmRef, path: &str, bytes: &[u8]) -> Result<(), VmError>;
    async fn destroy(&self, vm: &VmRef) -> Result<(), VmError>;
    async fn owner_of(&self, vm: &VmRef) -> Result<Option<PluginId>, VmError>;
    async fn reap(&self, installation_id: &str, current_epoch: u64) -> Result<(), VmError>;
    async fn shutdown(&self, installation_id: &str, session_epoch: u64) -> Result<(), VmError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OciReference {
    pub registry: String,
    pub repository: String,
    pub tag: Option<String>,
    pub digest: Option<String>,
}

impl OciReference {
    pub fn parse(value: &str) -> Result<Self, VmError> {
        if value.starts_with('/')
            || value.starts_with("./")
            || value.starts_with("../")
            || value.contains('\\')
        {
            return Err(invalid_oci_reference(value));
        }

        let (registry, remainder) = value
            .split_once('/')
            .ok_or_else(|| invalid_oci_reference(value))?;
        let registry = parse_registry(registry).ok_or_else(|| invalid_oci_reference(value))?;

        let (repository, tag, digest) =
            if let Some((repository, digest)) = remainder.split_once('@') {
                if repository.contains('@')
                    || digest.contains('@')
                    || !valid_repository(repository)
                    || !valid_digest(digest)
                {
                    return Err(invalid_oci_reference(value));
                }
                (repository, None, Some(digest.to_string()))
            } else {
                let (repository, tag) = remainder
                    .rsplit_once(':')
                    .ok_or_else(|| invalid_oci_reference(value))?;
                if !valid_repository(repository) || !valid_tag(tag) {
                    return Err(invalid_oci_reference(value));
                }
                (repository, Some(tag.to_string()), None)
            };

        Ok(Self {
            registry,
            repository: repository.to_string(),
            tag,
            digest,
        })
    }

    pub fn resolve(&self, settings: &VmSettings) -> Result<ResolvedImage, VmError> {
        if !settings
            .registries
            .iter()
            .any(|registry| registry == &self.registry)
        {
            return Err(VmError::Denied(format!(
                "OCI registry `{}` is not allowed",
                self.registry
            )));
        }
        Ok(ResolvedImage {
            registry: self.registry.clone(),
            repository: self.repository.clone(),
            tag: self.tag.clone(),
            digest: self.digest.clone(),
        })
    }

    fn canonical(&self) -> String {
        match (&self.tag, &self.digest) {
            (Some(tag), None) => format!("{}/{}:{tag}", self.registry, self.repository),
            (None, Some(digest)) => format!("{}/{}@{digest}", self.registry, self.repository),
            _ => unreachable!("OciReference construction preserves one selector"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedImage {
    pub registry: String,
    pub repository: String,
    pub tag: Option<String>,
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct VmSettings {
    pub registries: Vec<String>,
    pub limits: VmLimits,
    pub instance: VmInstanceSettings,
    pub calls: VmCallSettings,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct VmLimits {
    pub max_vms_per_plugin: u32,
}

impl Default for VmLimits {
    fn default() -> Self {
        Self {
            max_vms_per_plugin: 8,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct VmInstanceSettings {
    pub cpus: u32,
    pub memory_mb: u64,
    pub max_lifetime_ms: u64,
    pub idle_timeout_ms: u64,
}

impl Default for VmInstanceSettings {
    fn default() -> Self {
        Self {
            cpus: 1,
            memory_mb: 512,
            max_lifetime_ms: 3_600_000,
            idle_timeout_ms: 300_000,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct VmCallSettings {
    pub create_timeout_ms: u64,
    pub destroy_timeout_ms: u64,
    pub exec_timeout_ceiling_ms: u64,
    pub exec_max_output_bytes: u64,
    pub read_file_max_bytes: u64,
}

impl Default for VmCallSettings {
    fn default() -> Self {
        Self {
            create_timeout_ms: 600_000,
            destroy_timeout_ms: 60_000,
            exec_timeout_ceiling_ms: 120_000,
            exec_max_output_bytes: 64 * 1024,
            read_file_max_bytes: 16 * 1024 * 1024,
        }
    }
}

fn parse_registry(value: &str) -> Option<String> {
    if value.is_empty()
        || value.bytes().any(|byte| {
            !byte.is_ascii_alphanumeric() && !matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        })
    {
        return None;
    }

    let normalized = value.to_ascii_lowercase();
    if let Some(address) = normalized.strip_prefix('[') {
        let (address, port) = address.split_once(']')?;
        address.parse::<std::net::Ipv6Addr>().ok()?;
        if port.is_empty() || valid_registry_port(port.strip_prefix(':')?) {
            return Some(normalized);
        }
        return None;
    }

    let (host, port) = match normalized.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (normalized.as_str(), None),
    };
    if port.is_some_and(|port| !valid_registry_port(port))
        || !(host == "localhost" || host.contains('.'))
        || !host.split('.').all(|label| {
            label
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
                && label
                    .bytes()
                    .last()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric())
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return None;
    }
    Some(normalized)
}

fn valid_registry_port(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u16>().is_ok()
}

fn valid_repository(value: &str) -> bool {
    !value.is_empty()
        && value.split('/').all(|component| {
            component
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                && component
                    .bytes()
                    .last()
                    .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                && component.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'.' | b'_' | b'-')
                })
        })
}

fn valid_tag(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn invalid_oci_reference(value: &str) -> VmError {
    VmError::Failed(format!("invalid OCI image reference `{value}`"))
}

fn hash_component(hash: &mut Sha256, value: &[u8]) {
    hash_len(hash, value.len());
    hash.update(value);
}

fn hash_len(hash: &mut Sha256, len: usize) {
    hash.update((len as u64).to_be_bytes());
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = digest.as_ref();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use core::str::FromStr;
    use std::{format, vec};

    use super::{
        EnvVar, MountSpec, OciReference, RequestedVmConfig, VmCallSettings, VmError, VmIdentity,
        VmInstanceSettings, VmLimits, VmSettings,
    };
    use crate::vm::Egress;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn identity() -> VmIdentity {
        VmIdentity {
            installation_id: "installation-a".into(),
            session_epoch: 7,
            plugin_id: "build-plugin@grant-a".parse().unwrap(),
            logical_name: "build-env".into(),
        }
    }

    fn image() -> OciReference {
        OciReference::parse("ghcr.io/acme/build:1.2").unwrap()
    }

    fn mount(host: &str, guest: &str, readonly: bool) -> MountSpec {
        MountSpec {
            host: host.into(),
            guest: guest.into(),
            readonly,
        }
    }

    fn egress(value: &str) -> Egress {
        Egress::from_str(value).unwrap()
    }

    fn env(name: &str, value: &str) -> EnvVar {
        EnvVar {
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn physical_labels_are_deterministic_and_bind_every_identity_component() {
        let original = identity();
        assert_eq!(original.physical_label(), identity().physical_label());
        assert_eq!(original.physical_label().len(), 64);

        let variants = [
            VmIdentity {
                installation_id: "installation-b".into(),
                ..original.clone()
            },
            VmIdentity {
                session_epoch: 8,
                ..original.clone()
            },
            VmIdentity {
                plugin_id: "build-plugin@grant-b".parse().unwrap(),
                ..original.clone()
            },
            VmIdentity {
                logical_name: "test-env".into(),
                ..original.clone()
            },
        ];
        for variant in variants {
            assert_ne!(original.physical_label(), variant.physical_label());
        }
    }

    #[test]
    fn requested_config_hash_is_order_independent() {
        let first = RequestedVmConfig::normalized(
            image(),
            vec![
                mount("/project//src/", "/work/src", true),
                mount("/project/cache", "/cache", false),
            ],
            vec![egress("199.232.0.0/16:443"), egress("10.0.0.1:80")],
            vec![env("RUST_LOG", "info"), env("CI", "true")],
        )
        .unwrap();
        let shuffled = RequestedVmConfig::normalized(
            image(),
            vec![
                mount("/project/cache", "/cache", false),
                mount("/project/src", "/work//src/", true),
            ],
            vec![egress("10.0.0.1:80"), egress("199.232.0.0/16:443")],
            vec![env("CI", "true"), env("RUST_LOG", "info")],
        )
        .unwrap();

        assert_eq!(first, shuffled);
        assert_eq!(first.canonical_hash(), shuffled.canonical_hash());
    }

    #[test]
    fn requested_config_hash_changes_with_a_value() {
        let first = RequestedVmConfig::normalized(
            image(),
            vec![mount("/project/src", "/work/src", true)],
            vec![egress("199.232.0.0/16:443")],
            vec![env("RUST_LOG", "info")],
        )
        .unwrap();
        let changed = RequestedVmConfig::normalized(
            image(),
            vec![mount("/project/src", "/work/src", true)],
            vec![egress("199.232.0.0/16:443")],
            vec![env("RUST_LOG", "debug")],
        )
        .unwrap();

        assert_ne!(first.canonical_hash(), changed.canonical_hash());
    }

    #[test]
    fn requested_config_rejects_mount_and_environment_conflicts() {
        let mount_error = RequestedVmConfig::normalized(
            image(),
            vec![
                mount("/project/src", "/work/src", true),
                mount("/other/src", "/work/src", true),
            ],
            vec![],
            vec![],
        )
        .unwrap_err();
        assert!(matches!(mount_error, VmError::Failed(_)));

        let env_error = RequestedVmConfig::normalized(
            image(),
            vec![],
            vec![],
            vec![env("RUST_LOG", "info"), env("RUST_LOG", "debug")],
        )
        .unwrap_err();
        assert!(matches!(env_error, VmError::Failed(_)));
    }

    #[test]
    fn requested_config_collapses_exact_duplicates() {
        let config = RequestedVmConfig::normalized(
            image(),
            vec![
                mount("/project/src", "/work/src", true),
                mount("/project//src/", "/work//src/", true),
            ],
            vec![egress("199.232.0.0/16:443"), egress("199.232.0.0/16:0443")],
            vec![env("CI", "true"), env("CI", "true")],
        )
        .unwrap();

        assert_eq!(config.mounts.len(), 1);
        assert_eq!(config.egress.len(), 1);
        assert_eq!(config.env.len(), 1);
    }

    #[test]
    fn oci_parser_accepts_registry_qualified_references() {
        let tagged = OciReference::parse("ghcr.io/acme/build:1.2").unwrap();
        assert_eq!(tagged.registry, "ghcr.io");
        assert_eq!(tagged.repository, "acme/build");
        assert_eq!(tagged.tag.as_deref(), Some("1.2"));
        assert_eq!(tagged.digest, None);

        let digest =
            OciReference::parse(&format!("registry.example.com/x@sha256:{DIGEST}")).unwrap();
        assert_eq!(digest.tag, None);
        assert_eq!(digest.digest, Some(format!("sha256:{DIGEST}")));
    }

    #[test]
    fn oci_parser_rejects_local_and_unqualified_inputs() {
        for invalid in [
            "/host/path",
            "./img",
            "../img",
            "plainname",
            "img:tag",
            "C:/host/image:tag",
        ] {
            assert!(
                OciReference::parse(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn oci_resolution_enforces_the_registry_allowlist() {
        let reference = image();
        assert!(matches!(
            reference.resolve(&VmSettings::default()),
            Err(VmError::Denied(_))
        ));

        let settings = VmSettings {
            registries: vec!["ghcr.io".into()],
            ..VmSettings::default()
        };
        let resolved = reference.resolve(&settings).unwrap();
        assert_eq!(resolved.registry, "ghcr.io");
        assert_eq!(resolved.repository, "acme/build");
        assert_eq!(resolved.tag.as_deref(), Some("1.2"));
    }

    #[test]
    fn vm_settings_defaults_are_bounded_and_default_deny() {
        assert_eq!(
            VmSettings::default(),
            VmSettings {
                registries: vec![],
                limits: VmLimits::default(),
                instance: VmInstanceSettings::default(),
                calls: VmCallSettings::default(),
            }
        );
        assert_eq!(VmLimits::default().max_vms_per_plugin, 8);
        assert_eq!(VmInstanceSettings::default().cpus, 1);
        assert_eq!(VmInstanceSettings::default().memory_mb, 512);
        assert_eq!(VmInstanceSettings::default().max_lifetime_ms, 3_600_000);
        assert_eq!(VmInstanceSettings::default().idle_timeout_ms, 300_000);
        assert_eq!(VmCallSettings::default().create_timeout_ms, 600_000);
        assert_eq!(VmCallSettings::default().destroy_timeout_ms, 60_000);
        assert_eq!(VmCallSettings::default().exec_timeout_ceiling_ms, 120_000);
        assert_eq!(VmCallSettings::default().exec_max_output_bytes, 64 * 1024);
        assert_eq!(
            VmCallSettings::default().read_file_max_bytes,
            16 * 1024 * 1024
        );
    }
}
