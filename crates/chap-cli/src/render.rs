use chap_core::{
    ConsentError, ExportDriftKind, LoadError, PluginRefusal, PluginRefusalReason, SessionError,
    StartError,
};
use std::path::Path;

pub(crate) fn render_load_error(error: LoadError) -> String {
    match error {
        LoadError::CapabilityUnsupported { capability } => format!(
            "agent.{capability} is configured, but this build lacks {capability} support; rebuild with the `{capability}` feature"
        ),
        error => error.to_string(),
    }
}

pub(crate) fn render_start_error(error: StartError) -> String {
    match error {
        StartError::AdmissionRefused(refusals) => render_plugin_refusals(&refusals),
        error => error.to_string(),
    }
}

pub(crate) fn render_session_error(error: SessionError) -> String {
    match error {
        SessionError::Context(failures) => failures
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        error => error.to_string(),
    }
}

fn render_plugin_refusals(refusals: &[PluginRefusal]) -> String {
    refusals
        .iter()
        .map(render_plugin_refusal)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_plugin_refusal(refusal: &PluginRefusal) -> String {
    let plugin_id = &refusal.plugin_id;
    match &refusal.reason {
        PluginRefusalReason::ApprovalRequired => format!(
            "plugin `{plugin_id}` from `{}` requires approval before admission{}",
            refusal.source_path.display(),
            approval_remedy(plugin_id)
        ),
        PluginRefusalReason::RenewedApprovalRequired { drift } => format!(
            "plugin `{plugin_id}` from `{}` {} and requires renewed approval before admission{}",
            refusal.source_path.display(),
            drift_description(drift),
            approval_remedy(plugin_id)
        ),
        PluginRefusalReason::UnsupportedRole {
            role,
            exported_interfaces,
        } => render_unsupported_role(plugin_id, &refusal.source_path, role, exported_interfaces),
        PluginRefusalReason::RoleConfigInvalid { role } => format!(
            "plugin `{plugin_id}` configures a `{role}` section, but its component does not export the {role} interface"
        ),
        PluginRefusalReason::ComponentLoad { source } => source.to_string(),
        _ => refusal.to_string(),
    }
}

fn approval_remedy(plugin_id: &chap_core::PluginId) -> String {
    format!("; run `chap grants review {plugin_id}` and then `chap grants approve {plugin_id}`")
}

fn drift_description(drift: &chap_core::DriftReport) -> &'static str {
    if drift
        .export_changes
        .iter()
        .any(|change| change.kind == ExportDriftKind::Gained)
    {
        "now exports interfaces it was not approved for"
    } else if drift.blocks_admission {
        "expanded its permission manifest"
    } else {
        "reported a changed permission manifest"
    }
}

pub(crate) fn render_consent_error(error: ConsentError) -> String {
    match error {
        ConsentError::StateLocation(source) | ConsentError::HostConfiguration(source) => {
            render_load_error(source)
        }
        ConsentError::UnsupportedRole {
            plugin_id,
            path,
            role,
            exported_interfaces,
        } => render_unsupported_role(&plugin_id, &path, &role, &exported_interfaces),
        error => error.to_string(),
    }
}

fn render_unsupported_role(
    plugin_id: &chap_core::PluginId,
    path: &Path,
    role: &str,
    exported_interfaces: &[String],
) -> String {
    format!(
        "plugin `{plugin_id}` from `{}` does not implement a supported role; expected an export from the `{role}` package, but the component exports {}",
        path.display(),
        render_exported_interfaces(exported_interfaces)
    )
}

fn render_exported_interfaces(interfaces: &[String]) -> String {
    if interfaces.is_empty() {
        return "no interfaces".to_owned();
    }
    interfaces
        .iter()
        .map(|interface| format!("`{interface}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chap_core::{
        AgentBuilder, ContextError, ContextFailure, DriftReport, ExportDrift, ExportDriftKind,
    };
    use std::{io, path::PathBuf};

    fn refusal(plugin_id: &str, component: &str, reason: PluginRefusalReason) -> PluginRefusal {
        PluginRefusal {
            plugin_id: plugin_id.parse().unwrap(),
            source_path: PathBuf::from(component),
            reason,
        }
    }

    #[test]
    fn renders_config_load_failure_lines() {
        assert_eq!(
            render_load_error(LoadError::ReadConfig {
                path: PathBuf::from("missing.json"),
                source: io::Error::new(io::ErrorKind::PermissionDenied, "permission denied"),
            }),
            "failed to read `missing.json`: permission denied"
        );
        assert_eq!(
            render_load_error(LoadError::ParseConfig {
                path: PathBuf::from("broken.json"),
                source: serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
            }),
            "failed to parse `broken.json`: EOF while parsing an object at line 1 column 1"
        );
        assert_eq!(
            render_load_error(LoadError::ResolveConfigPath {
                path: PathBuf::from("relative/chap.json"),
                source: io::Error::new(io::ErrorKind::NotFound, "current directory missing"),
            }),
            "failed to resolve config path `relative/chap.json` as an absolute path: current directory missing"
        );
        assert_eq!(
            render_load_error(LoadError::InvalidName {
                name: "team/alpha".to_owned(),
            }),
            "invalid instance name `team/alpha`: expected a non-empty value containing only A-Z, a-z, 0-9, '.', '_', or '-', other than '.' or '..'"
        );
        assert_eq!(
            render_load_error(LoadError::StateDirectoryUnavailable),
            "cannot locate CHAP state: neither XDG_STATE_HOME nor HOME is set to a non-empty value"
        );
        assert_eq!(
            render_load_error(LoadError::InvalidAgentConfigSection {
                section: "agent.exec",
                source: serde_json::from_str::<u64>(r#""soon""#).unwrap_err(),
            }),
            "failed to parse the `agent.exec` section: invalid type: string \"soon\", expected u64 at line 1 column 6"
        );
    }

    #[test]
    fn renders_the_exec_rebuild_hint() {
        assert_eq!(
            render_load_error(LoadError::CapabilityUnsupported { capability: "exec" }),
            "agent.exec is configured, but this build lacks exec support; rebuild with the `exec` feature"
        );
    }

    #[test]
    fn renders_context_failures_one_per_line() {
        let error = SessionError::Context(vec![
            ContextFailure {
                plugin_id: "alpha".parse().unwrap(),
                source: ContextError::PluginReported {
                    message: "unavailable".to_owned(),
                },
            },
            ContextFailure {
                plugin_id: "bravo".parse().unwrap(),
                source: ContextError::PluginReported {
                    message: "timed out after 10s".to_owned(),
                },
            },
        ]);

        assert_eq!(
            render_session_error(error),
            "context plugin `alpha` failed: unavailable\ncontext plugin `bravo` failed: timed out after 10s"
        );
        assert_eq!(
            render_session_error(SessionError::ProviderNotConfigured {
                provider: "missing".parse().unwrap(),
            }),
            "provider plugin `missing` is not configured"
        );
    }

    #[test]
    fn renders_admission_remedies_one_per_line() {
        let refusals = [
            refusal(
                "alpha",
                "plugins/alpha.wasm",
                PluginRefusalReason::ApprovalRequired,
            ),
            refusal(
                "bravo",
                "plugins/bravo.wasm",
                PluginRefusalReason::RenewedApprovalRequired {
                    drift: DriftReport {
                        changes: Vec::new(),
                        export_changes: Vec::new(),
                        blocks_admission: true,
                    },
                },
            ),
        ];

        assert_eq!(
            render_start_error(StartError::AdmissionRefused(refusals.into())),
            "plugin `alpha` from `plugins/alpha.wasm` requires approval before admission; run `chap grants review alpha` and then `chap grants approve alpha`\n\
             plugin `bravo` from `plugins/bravo.wasm` expanded its permission manifest and requires renewed approval before admission; run `chap grants review bravo` and then `chap grants approve bravo`"
        );
    }

    #[test]
    fn renders_gained_exports_as_renewed_approval() {
        let refusal = refusal(
            "example",
            "plugins/example.wasm",
            PluginRefusalReason::RenewedApprovalRequired {
                drift: DriftReport {
                    changes: Vec::new(),
                    export_changes: vec![ExportDrift {
                        name: "chap:agent/tools".to_owned(),
                        kind: ExportDriftKind::Gained,
                        before: Vec::new(),
                        after: vec!["chap:agent/tools@0.3.0".to_owned()],
                    }],
                    blocks_admission: true,
                },
            },
        );

        assert_eq!(
            render_plugin_refusal(&refusal),
            "plugin `example` from `plugins/example.wasm` now exports interfaces it was not approved for and requires renewed approval before admission; run `chap grants review example` and then `chap grants approve example`"
        );
    }

    #[test]
    fn renders_nonblocking_drift_as_renewed_approval() {
        let refusal = refusal(
            "example",
            "plugins/example.wasm",
            PluginRefusalReason::RenewedApprovalRequired {
                drift: DriftReport {
                    changes: Vec::new(),
                    export_changes: Vec::new(),
                    blocks_admission: false,
                },
            },
        );

        assert_eq!(
            render_plugin_refusal(&refusal),
            "plugin `example` from `plugins/example.wasm` reported a changed permission manifest and requires renewed approval before admission; run `chap grants review example` and then `chap grants approve example`"
        );
    }

    #[test]
    fn renders_structured_unsupported_role_details() {
        let refusal = refusal(
            "example",
            "plugins/example.wasm",
            PluginRefusalReason::UnsupportedRole {
                role: "chap:agent@0.3.0".to_owned(),
                exported_interfaces: vec![
                    "lockgate:config/schema".to_owned(),
                    "example:plugin/unsupported@1.0.0".to_owned(),
                ],
            },
        );

        assert_eq!(
            render_plugin_refusal(&refusal),
            "plugin `example` from `plugins/example.wasm` does not implement a supported role; expected an export from the `chap:agent@0.3.0` package, but the component exports `lockgate:config/schema`, `example:plugin/unsupported@1.0.0`"
        );
    }

    #[test]
    fn renders_structured_role_config_and_component_failures() {
        let role = refusal(
            "example",
            "plugins/example.wasm",
            PluginRefusalReason::RoleConfigInvalid {
                role: "tools".to_owned(),
            },
        );
        let load = refusal(
            "missing",
            "plugins/missing.wasm",
            PluginRefusalReason::ComponentLoad {
                source: Box::new(ConsentError::ReadPlugin {
                    plugin_id: "missing".parse().unwrap(),
                    path: PathBuf::from("plugins/missing.wasm"),
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, "component missing"),
                }),
            },
        );

        assert_eq!(
            render_plugin_refusal(&role),
            "plugin `example` configures a `tools` section, but its component does not export the tools interface"
        );
        assert_eq!(
            render_plugin_refusal(&load),
            "failed to read plugin `missing` from `plugins/missing.wasm`: component missing"
        );
    }

    #[tokio::test]
    async fn renders_load_plugin_component_failures() {
        let directory = tempfile::tempdir().unwrap();
        let component = directory.path().join("broken.wasm");
        std::fs::write(&component, []).unwrap();
        let config_path = directory.path().join("chap.json");
        std::fs::write(
            &config_path,
            r#"{
                "plugins": {
                    "broken": {
                        "component": "broken.wasm"
                    }
                }
            }"#,
        )
        .unwrap();
        let builder = AgentBuilder::load(&config_path)
            .unwrap()
            .state_dir(directory.path());
        let error = builder
            .review_plugin(&"broken".parse().unwrap())
            .await
            .unwrap_err();

        assert!(matches!(
            &error,
            ConsentError::LoadPlugin { plugin_id, path, .. }
                if plugin_id.as_str() == "broken" && path == &component
        ));
        let refusal = PluginRefusal {
            plugin_id: "broken".parse().unwrap(),
            source_path: component.clone(),
            reason: PluginRefusalReason::ComponentLoad {
                source: Box::new(error),
            },
        };

        assert_eq!(
            render_plugin_refusal(&refusal),
            format!(
                "failed to load plugin `broken` from `{}`: input is not a valid WebAssembly component: unexpected end-of-file (at offset 0x0)",
                component.display()
            )
        );
    }

    #[test]
    fn renders_consent_errors_with_export_details() {
        let error = ConsentError::UnsupportedRole {
            plugin_id: "example".parse().unwrap(),
            path: PathBuf::from("plugins/example.wasm"),
            role: "chap:agent@0.3.0".to_owned(),
            exported_interfaces: Vec::new(),
        };

        assert_eq!(
            render_consent_error(error),
            "plugin `example` from `plugins/example.wasm` does not implement a supported role; expected an export from the `chap:agent@0.3.0` package, but the component exports no interfaces"
        );
    }
}
