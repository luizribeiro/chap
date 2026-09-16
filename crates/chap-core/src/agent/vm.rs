use std::{format, path::Path, str::FromStr, string::ToString, vec, vec::Vec};

use chap_vm::{
    host::{EnvVar, MountSpec, OciReference, RequestedVmConfig, VmRef, VmSettings},
    vm::{Egress, InstanceScope, Mount},
};
use lockgate::{PluginSubject, ScopedResource};

use super::bindings::vm;

pub(super) struct MountResource(pub(super) Mount);

impl MountResource {
    fn scopes(&self) -> Vec<Mount> {
        vec![self.0.clone()]
    }
}

impl ScopedResource<Mount> for MountResource {
    fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<Mount> {
        self.scopes()
    }
}

pub(super) struct EgressResource(pub(super) Egress);

impl EgressResource {
    fn scopes(&self) -> Vec<Egress> {
        vec![self.0.clone()]
    }
}

impl ScopedResource<Egress> for EgressResource {
    fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<Egress> {
        self.scopes()
    }
}

pub(super) struct ManagedVm {
    pub(super) vm_ref: VmRef,
}

impl ScopedResource<InstanceScope> for ManagedVm {
    fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<InstanceScope> {
        vec![InstanceScope::CreatedByCaller]
    }
}

pub(super) fn translate_requested_config(
    config: &vm::VmConfig,
    _settings: &VmSettings,
) -> Result<(RequestedVmConfig, Vec<MountResource>, Vec<EgressResource>), vm::VmError> {
    let image = OciReference::parse(&config.image).map_err(translate_host_error)?;
    let mounts = config
        .mounts
        .iter()
        .map(|mount| {
            let host = std::fs::canonicalize(Path::new(&mount.host)).map_err(|error| {
                vm::VmError::Failed(format!(
                    "failed to resolve VM mount host path `{}`: {error}",
                    mount.host
                ))
            })?;
            let host = host.into_os_string().into_string().map_err(|_| {
                vm::VmError::Failed(format!(
                    "VM mount host path `{}` resolves to a non-UTF-8 path",
                    mount.host
                ))
            })?;
            Ok::<_, vm::VmError>(MountSpec {
                host,
                guest: mount.guest.clone(),
                readonly: mount.readonly,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let egress = config
        .egress
        .iter()
        .map(|destination| {
            Egress::from_str(destination).map_err(|error| vm::VmError::Failed(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let env = config
        .env
        .iter()
        .map(|(name, value)| EnvVar {
            name: name.clone(),
            value: value.clone(),
        })
        .collect();

    let requested =
        RequestedVmConfig::normalized(image, mounts, egress, env).map_err(translate_host_error)?;
    let mount_resources = requested
        .mounts
        .iter()
        .map(|mount| {
            Mount::new(&mount.host, mount.readonly)
                .map(MountResource)
                .map_err(|error| vm::VmError::Failed(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let egress_resources = requested
        .egress
        .iter()
        .cloned()
        .map(EgressResource)
        .collect();

    Ok((requested, mount_resources, egress_resources))
}

pub(super) fn translate_host_error(error: chap_vm::host::VmError) -> vm::VmError {
    match error {
        chap_vm::host::VmError::AlreadyExists => vm::VmError::AlreadyExists,
        chap_vm::host::VmError::NoSuchVm => vm::VmError::NoSuchVm,
        chap_vm::host::VmError::ConfigMismatch => vm::VmError::ConfigMismatch,
        chap_vm::host::VmError::TimedOut => vm::VmError::TimedOut,
        chap_vm::host::VmError::Denied(message) => vm::VmError::Denied(message),
        chap_vm::host::VmError::Failed(message) | chap_vm::host::VmError::Unavailable(message) => {
            vm::VmError::Failed(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use lockgate::{Scope, ScopeRepr};

    use super::*;

    fn config() -> vm::VmConfig {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        vm::VmConfig {
            image: "ghcr.io/acme/build:1.2".into(),
            mounts: vec![
                vm::Mount {
                    host: format!("{}//", manifest.join("src").display()),
                    guest: "/work//src/".into(),
                    readonly: true,
                },
                vm::Mount {
                    host: manifest.join("tests").to_string_lossy().into_owned(),
                    guest: "/cache".into(),
                    readonly: false,
                },
            ],
            egress: vec!["199.232.0.0/16:443".into(), "10.0.0.1:80".into()],
            env: vec![
                ("RUST_LOG".into(), "info".into()),
                ("CI".into(), "true".into()),
            ],
        }
    }

    #[test]
    fn translates_a_realistic_requested_config_and_scope_resources() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let source = manifest.join("src").canonicalize().unwrap();
        let tests = manifest.join("tests").canonicalize().unwrap();
        let (requested, mounts, egress) =
            translate_requested_config(&config(), &VmSettings::default()).unwrap();

        assert_eq!(requested.image.registry, "ghcr.io");
        assert_eq!(requested.image.repository, "acme/build");
        assert_eq!(requested.image.tag.as_deref(), Some("1.2"));
        assert_eq!(requested.image.digest, None);
        assert_eq!(
            requested.mounts,
            [
                MountSpec {
                    host: tests.to_string_lossy().into_owned(),
                    guest: "/cache".into(),
                    readonly: false,
                },
                MountSpec {
                    host: source.to_string_lossy().into_owned(),
                    guest: "/work/src".into(),
                    readonly: true,
                },
            ]
        );
        assert_eq!(
            requested
                .egress
                .iter()
                .map(ScopeRepr::canonical)
                .collect::<Vec<_>>(),
            ["10.0.0.1:80", "199.232.0.0/16:443"]
        );
        assert_eq!(
            requested.env,
            [
                EnvVar {
                    name: "CI".into(),
                    value: "true".into(),
                },
                EnvVar {
                    name: "RUST_LOG".into(),
                    value: "info".into(),
                },
            ]
        );
        assert_eq!(
            mounts
                .iter()
                .flat_map(MountResource::scopes)
                .map(|scope| scope.canonical())
                .collect::<Vec<_>>(),
            [
                format!("rw:{}", tests.display()),
                format!("ro:{}", source.display()),
            ]
        );
        assert_eq!(
            egress
                .iter()
                .flat_map(EgressResource::scopes)
                .map(|scope| scope.canonical())
                .collect::<Vec<_>>(),
            ["10.0.0.1:80", "199.232.0.0/16:443"]
        );
    }

    #[test]
    fn rejects_a_local_path_image() {
        let mut config = config();
        config.image = "/host/etc".into();

        assert!(matches!(
            translate_requested_config(&config, &VmSettings::default()),
            Err(vm::VmError::Failed(_))
        ));
    }

    #[test]
    fn rejects_malformed_egress() {
        let mut config = config();
        config.egress = vec!["not-an-ip".into()];

        assert!(matches!(
            translate_requested_config(&config, &VmSettings::default()),
            Err(vm::VmError::Failed(_))
        ));
    }

    #[test]
    fn rejects_conflicting_mounts() {
        let mut config = config();
        config.mounts.push(vm::Mount {
            host: env!("CARGO_MANIFEST_DIR").into(),
            guest: "/work/src".into(),
            readonly: false,
        });

        assert!(matches!(
            translate_requested_config(&config, &VmSettings::default()),
            Err(vm::VmError::Failed(message)) if message.contains("conflicting mounts")
        ));
    }

    #[test]
    fn reports_a_missing_mount_host_path_as_a_failure() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing");
        let mut config = config();
        config.mounts = vec![vm::Mount {
            host: missing.to_string_lossy().into_owned(),
            guest: "/mnt/missing".into(),
            readonly: true,
        }];

        assert!(matches!(
            translate_requested_config(&config, &VmSettings::default()),
            Err(vm::VmError::Failed(message)) if message.contains("failed to resolve VM mount host path")
        ));
    }

    #[test]
    fn resource_scopes_return_the_expected_witnesses() {
        let mount = MountResource(Mount::from_str("ro:/project/src").unwrap());
        let egress = EgressResource(Egress::from_str("10.0.0.1:443").unwrap());

        assert_eq!(
            mount
                .scopes()
                .iter()
                .map(ScopeRepr::canonical)
                .collect::<Vec<_>>(),
            ["ro:/project/src"]
        );
        assert_eq!(
            egress
                .scopes()
                .iter()
                .map(ScopeRepr::canonical)
                .collect::<Vec<_>>(),
            ["10.0.0.1:443"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn canonical_mount_witness_denies_a_symlink_escape_and_allows_a_real_subdirectory() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let granted = root.path().join("granted");
        let sibling = root.path().join("sibling");
        let subdirectory = granted.join("subdirectory");
        std::fs::create_dir(&granted).unwrap();
        std::fs::create_dir(&sibling).unwrap();
        std::fs::create_dir(&subdirectory).unwrap();
        symlink(&sibling, granted.join("link")).unwrap();
        let granted = granted.canonicalize().unwrap();
        let grant = Mount::from_str(&format!("rw:{}", granted.to_str().unwrap())).unwrap();

        let mut escaped = config();
        escaped.mounts = vec![vm::Mount {
            host: granted.join("link").to_string_lossy().into_owned(),
            guest: "/mnt/escape".into(),
            readonly: false,
        }];
        escaped.egress.clear();
        let (escaped, witnesses, _) =
            translate_requested_config(&escaped, &VmSettings::default()).unwrap();

        assert_eq!(
            escaped.mounts[0].host,
            sibling.canonicalize().unwrap().to_string_lossy()
        );
        assert!(!grant.contains(&witnesses[0].0), "vm.mount must deny");

        let mut allowed = config();
        allowed.mounts = vec![vm::Mount {
            host: subdirectory.to_string_lossy().into_owned(),
            guest: "/mnt/subdirectory".into(),
            readonly: false,
        }];
        allowed.egress.clear();
        let (_, witnesses, _) =
            translate_requested_config(&allowed, &VmSettings::default()).unwrap();

        assert!(grant.contains(&witnesses[0].0));
    }
}
