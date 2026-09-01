use std::{format, str::FromStr, string::ToString, vec, vec::Vec};

use chap_vm::{
    host::{EnvVar, MountSpec, OciReference, RequestedVmConfig, VmSettings},
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
    pub(super) owned_by_caller: bool,
}

impl ManagedVm {
    fn scopes(&self) -> Vec<InstanceScope> {
        if self.owned_by_caller {
            vec![InstanceScope::CreatedByCaller]
        } else {
            vec![]
        }
    }
}

impl ScopedResource<InstanceScope> for ManagedVm {
    fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<InstanceScope> {
        self.scopes()
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
        .map(|mount| MountSpec {
            host: mount.host.clone(),
            guest: mount.guest.clone(),
            readonly: mount.readonly,
        })
        .collect();
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
            let host = &mount.host;
            let witness = if mount.readonly {
                format!("ro:{host}")
            } else {
                host.clone()
            };
            Mount::from_str(&witness)
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
    use lockgate::ScopeRepr;

    use super::*;

    fn config() -> vm::VmConfig {
        vm::VmConfig {
            image: "ghcr.io/acme/build:1.2".into(),
            mounts: vec![
                vm::Mount {
                    host: "/project//src/".into(),
                    guest: "/work//src/".into(),
                    readonly: true,
                },
                vm::Mount {
                    host: "/project/cache".into(),
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
                    host: "/project/cache".into(),
                    guest: "/cache".into(),
                    readonly: false,
                },
                MountSpec {
                    host: "/project/src".into(),
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
            ["/project/cache", "ro:/project/src"]
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
            host: "/project/generated".into(),
            guest: "/work/src".into(),
            readonly: false,
        });

        assert!(matches!(
            translate_requested_config(&config, &VmSettings::default()),
            Err(vm::VmError::Failed(message)) if message.contains("conflicting mounts")
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

    #[test]
    fn managed_vm_scopes_only_vms_owned_by_the_caller() {
        assert!(
            ManagedVm {
                owned_by_caller: false,
            }
            .scopes()
            .is_empty()
        );
        assert_eq!(
            ManagedVm {
                owned_by_caller: true,
            }
            .scopes(),
            [InstanceScope::CreatedByCaller]
        );
    }
}
