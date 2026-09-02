use super::{
    super::{
        AgentBuilder, PluginBudgets, host_builder,
        provider::{CompletionBackend, PluginBackend},
        runtime_limits,
    },
    fixtures::{
        fast_provider_component, fast_tool_component, hanging_provider_component,
        hanging_tool_component, provider_and_tool_component, provider_component,
        provider_component_requiring_env, provider_component_requiring_exec,
        provider_component_with_schema, provider_component_with_trapping_schema, test_directory,
        tool_component, tool_component_with_schema, unsupported_component,
    },
    load_test_builder,
};
use crate::{
    CallBudget, ConsentError, ConsentRecord, ExecutionMode, ExportDriftKind, FinishReason,
    PluginRefusal, PluginRefusalReason, ProviderError, StartError, Tool, ToolDefinition, ToolError,
    ToolRegistrationError,
};
use lockgate::{ConsentRequired, DriftReport, Role, RuntimeLimits};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

fn refused_plugins(error: StartError) -> Vec<PluginRefusal> {
    let StartError::AdmissionRefused(refusals) = error else {
        panic!("expected structured plugin refusals")
    };
    refusals
}

fn only_refusal(error: StartError) -> PluginRefusal {
    let mut refusals = refused_plugins(error);
    assert_eq!(refusals.len(), 1);
    refusals.pop().unwrap()
}

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

async fn panic_join_error() -> tokio::task::JoinError {
    tokio::spawn(async { panic!("forced cleanup panic") })
        .await
        .unwrap_err()
}

#[tokio::test]
async fn refused_plugin_cleanup_preserves_the_join_error() {
    let error = StartError::RefusedPluginCleanup {
        plugin: "example".to_owned(),
        source: panic_join_error().await,
    };

    assert!(matches!(
        &error,
        StartError::RefusedPluginCleanup { source, .. } if source.is_panic()
    ));
    assert!(
        std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<tokio::task::JoinError>())
            .filter(|source| source.is_panic())
            .is_some()
    );
}

#[tokio::test]
async fn operation_and_cleanup_preserves_the_cleanup_join_error() {
    let error = StartError::OperationAndCleanup {
        source: Box::new(StartError::RefusedPluginCleanup {
            plugin: "example".to_owned(),
            source: panic_join_error().await,
        }),
        cleanup: Box::new(ConsentError::HostCleanup {
            source: panic_join_error().await,
        }),
    };
    let StartError::OperationAndCleanup { cleanup, .. } = &error else {
        unreachable!()
    };

    assert!(matches!(
        cleanup.as_ref(),
        ConsentError::HostCleanup { source } if source.is_panic()
    ));
    assert!(
        std::error::Error::source(&error)
            .and_then(std::error::Error::source)
            .and_then(|source| source.downcast_ref::<tokio::task::JoinError>())
            .filter(|source| source.is_panic())
            .is_some()
    );
    assert!(
        std::error::Error::source(cleanup.as_ref())
            .and_then(|source| source.downcast_ref::<tokio::task::JoinError>())
            .filter(|source| source.is_panic())
            .is_some()
    );
}

#[test]
fn guest_http_request_ceiling_uses_its_own_limit() {
    assert_default_runtime_limits_except_timeout_ceiling(
        runtime_limits(),
        super::super::GUEST_HTTP_REQUEST_CEILING,
    );
}

#[test]
fn default_call_budgets_preserve_existing_bounds() {
    let budgets = PluginBudgets::default();

    assert_eq!(
        budgets.provider.complete,
        CallBudget {
            fuel: 25_000_000,
            deadline: Duration::from_secs(120),
        }
    );
    assert_eq!(
        budgets.tools.definitions,
        CallBudget {
            fuel: 25_000_000,
            deadline: Duration::from_secs(30),
        }
    );
    assert_eq!(
        budgets.tools.execute,
        CallBudget {
            fuel: 25_000_000,
            deadline: Duration::from_secs(30),
        }
    );
    assert_eq!(
        budgets.context.segments,
        CallBudget {
            fuel: 25_000_000,
            deadline: Duration::from_secs(10),
        }
    );
    assert_eq!(
        budgets.admission,
        CallBudget {
            fuel: 25_000_000,
            deadline: Duration::from_secs(30),
        }
    );
}

fn assert_default_runtime_limits_except_timeout_ceiling(
    limits: RuntimeLimits,
    timeout_ceiling: Duration,
) {
    let default = RuntimeLimits::default();

    assert_eq!(limits.http_request_timeout_ceiling, Some(timeout_ceiling));
    assert_eq!(limits.instantiation_fuel, default.instantiation_fuel);
    assert_eq!(limits.max_memory_bytes, default.max_memory_bytes);
    assert_eq!(limits.max_detached_jobs, default.max_detached_jobs);
}

#[cfg(not(feature = "exec"))]
#[tokio::test]
async fn rejects_exec_needs_when_the_capability_is_not_registered() {
    let (directory, config_path) = exec_plugin_config(&["cargo", "git commit"]);
    let builder = load_test_builder(&config_path);

    let error = builder.review_plugin("example").await.unwrap_err();

    assert!(matches!(
        error,
        ConsentError::LoadPlugin { ref plugin, .. } if plugin == "example"
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(feature = "exec")]
#[tokio::test]
async fn resolves_exec_setting_arrays_into_canonical_review_scopes() {
    let (directory, config_path) = exec_plugin_config(&["cargo", "git commit"]);
    let builder = load_test_builder(&config_path);

    let manifest = builder.review_plugin("example").await.unwrap().manifest;
    let grant = manifest
        .grants
        .iter()
        .find(|grant| grant.capability == "exec" && grant.permission == "run")
        .expect("the exec.run grant should be reviewable");

    assert_eq!(
        grant.scopes,
        vec!["cargo".to_owned(), "git commit".to_owned()]
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(feature = "exec")]
#[tokio::test]
async fn expanded_exec_settings_drift_and_block_readmission() {
    let (directory, config_path) = exec_plugin_config(&["cargo", "git commit"]);
    load_test_builder(&config_path)
        .approve_plugin("example")
        .await
        .unwrap();
    write_exec_plugin_config(&directory, &["cargo", "git commit", "rg"]);
    let builder = load_test_builder(&config_path);

    let review = builder.review_plugin("example").await.unwrap();
    let drift = review.drift.expect("the added command should cause drift");
    assert!(drift.blocks_admission);
    assert!(drift.changes.iter().any(|change| {
        change.capability == "exec"
            && change.permission == "run"
            && change
                .after
                .as_ref()
                .is_some_and(|scopes| scopes.iter().any(|scope| scope == "rg"))
    }));

    let error = builder.start().await.err().unwrap();
    let refusal = only_refusal(error);
    assert_eq!(refusal.instance_id, "example");
    let PluginRefusalReason::RenewedApprovalRequired { drift } = refusal.reason else {
        panic!("expected renewed approval")
    };
    assert!(drift.blocks_admission);
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(feature = "exec")]
#[tokio::test]
async fn rejects_an_empty_required_exec_setting_array() {
    let (directory, config_path) = exec_plugin_config(&[]);
    let builder = load_test_builder(&config_path);

    let error = builder.review_plugin("example").await.unwrap_err();

    assert!(matches!(
        error,
        ConsentError::LoadPlugin { ref plugin, .. } if plugin == "example"
    ));
    fs::remove_dir_all(directory).unwrap();
}

fn exec_plugin_config(allowed_commands: &[&str]) -> (PathBuf, PathBuf) {
    let directory = test_directory();
    let config_path = directory.join("chap.json");
    write_exec_plugin_config(&directory, allowed_commands);
    (directory, config_path)
}

fn write_exec_plugin_config(directory: &Path, allowed_commands: &[&str]) {
    fs::write(
        directory.join("provider.wasm"),
        provider_component_requiring_exec("example.provider"),
    )
    .unwrap();
    let config = serde_json::json!({
        "plugins": {
            "example": {
                "component": "provider.wasm",
                "settings": {
                    "allowed_commands": allowed_commands,
                }
            }
        }
    });
    fs::write(
        directory.join("chap.json"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn reviews_multiple_plugins_with_one_caller_owned_host() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "alpha": { "component": "provider.wasm" },
                "bravo": { "component": "provider.wasm" }
            }
        }"#,
    )
    .unwrap();
    let builder = load_test_builder(&config_path);
    let consent = builder.consent_store().unwrap();
    let mut host = host_builder(&builder.config, builder.budgets)
        .await
        .unwrap();

    let reviews = builder
        .review_configured_plugins(&mut host, &consent, &["alpha", "bravo"])
        .await
        .unwrap();

    assert_eq!(reviews.len(), 2);
    assert_eq!(reviews[0].manifest.instance_id, "alpha");
    assert_eq!(reviews[1].manifest.instance_id, "bravo");
    tokio::task::spawn_blocking(move || drop(host))
        .await
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn approved_matching_manifest_admits_a_configured_provider() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(
        &component,
        provider_component_with_schema(
            "example.provider",
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"model":{"type":"string"}},"required":["model"],"additionalProperties":false}"#,
        ),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example.provider": {
                    "component": "provider.wasm",
                    "settings": {
                        "model": "example-model"
                    }
                }
            }
        }"#,
    )
    .unwrap();

    let builder = load_test_builder(&config_path);

    assert_eq!(builder.plugins().count(), 1);
    assert_eq!(
        builder.plugin_roles("example.provider").unwrap(),
        ["provider"]
    );
    let record = builder.approve_plugin("example.provider").await.unwrap();
    assert_eq!(
        fs::read(directory.join("config-path")).unwrap(),
        fs::canonicalize(&config_path)
            .unwrap()
            .as_os_str()
            .as_encoded_bytes()
    );
    assert!(
        time::OffsetDateTime::parse(
            &record.approved_at,
            &time::format_description::well_known::Rfc3339
        )
        .is_ok()
    );
    let agent = builder.start().await.unwrap();
    assert!(agent.inner.plugins.contains_key("example.provider"));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn classifies_a_trapping_provider_as_a_plugin_failure() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example": {
                    "component": "provider.wasm"
                }
            }
        }"#,
    )
    .unwrap();
    let builder = load_test_builder(&config_path);
    builder.approve_plugin("example").await.unwrap();
    let agent = builder.start().await.unwrap();
    let backend = PluginBackend::new(&agent.inner, "example");

    let error = backend.complete(Vec::new()).await.unwrap_err();

    let ProviderError::CallFailed { provider, source } = error else {
        panic!("a trapping provider should be a plugin failure");
    };
    assert_eq!(provider, "example");
    assert!(matches!(
        source.as_ref(),
        lockgate::CallError::Trap { detail } if !detail.is_empty()
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn configured_provider_budget_reaches_the_host_builder() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        hanging_provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "agent": {
                "budgets": {
                    "provider": { "deadline_ms": 1000 }
                }
            },
            "plugins": {
                "example": {
                    "component": "provider.wasm"
                }
            }
        }"#,
    )
    .unwrap();
    let builder = load_test_builder(&config_path);
    builder.approve_plugin("example").await.unwrap();
    let agent = builder.start().await.unwrap();
    let backend = PluginBackend::new(&agent.inner, "example");

    let error = tokio::time::timeout(Duration::from_secs(5), backend.complete(Vec::new()))
        .await
        .expect("hanging provider call did not respect its deadline")
        .unwrap_err();

    let ProviderError::TimedOut {
        provider,
        deadline,
        source,
    } = &error
    else {
        panic!("a provider deadline must remain a timed-out failure")
    };
    assert_eq!(provider, "example");
    assert_eq!(*deadline, Duration::from_secs(1));
    assert!(matches!(
        source.as_ref(),
        lockgate::CallError::DeadlineExceeded { deadline }
            if *deadline == Duration::from_secs(1)
    ));
    assert_eq!(
        error.to_string(),
        "provider plugin `example` timed out after 1s"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn fast_provider_and_tool_plugins_succeed_with_deadlines() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        fast_provider_component("example.provider"),
    )
    .unwrap();
    fs::write(
        directory.join("tools.wasm"),
        fast_tool_component("example.tools"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example.provider": {
                    "component": "provider.wasm"
                },
                "example.tools": {
                    "component": "tools.wasm"
                }
            }
        }"#,
    )
    .unwrap();
    let builder = load_test_builder(&config_path);
    builder.approve_plugin("example.provider").await.unwrap();
    builder.approve_plugin("example.tools").await.unwrap();
    let agent = builder.start().await.unwrap();
    let backend = PluginBackend::new(&agent.inner, "example.provider");

    let completion = backend.complete(Vec::new()).await.unwrap();
    let tool_output = agent
        .inner
        .tools
        .execute("fixture-tool", "{}".to_owned())
        .await
        .unwrap();

    assert!(completion.content.is_empty());
    assert!(matches!(completion.finish_reason, FinishReason::Stop));
    assert!(completion.usage.is_none());
    assert!(tool_output.is_empty());
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn start_refuses_and_names_every_unapproved_plugin() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "alpha-unapproved": {
                    "component": "provider.wasm"
                },
                "approved": {
                    "component": "provider.wasm"
                },
                "bravo-unapproved": {
                    "component": "provider.wasm"
                }
            }
        }"#,
    )
    .unwrap();

    let builder = load_test_builder(&config_path);
    builder.approve_plugin("approved").await.unwrap();
    let error = builder.start().await.err().unwrap();

    let refusals = refused_plugins(error);
    assert_eq!(refusals.len(), 2);
    assert_eq!(refusals[0].instance_id, "alpha-unapproved");
    assert!(matches!(
        refusals[0].reason,
        PluginRefusalReason::ApprovalRequired
    ));
    assert_eq!(refusals[1].instance_id, "bravo-unapproved");
    assert!(matches!(
        refusals[1].reason,
        PluginRefusalReason::ApprovalRequired
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn coherence_check_accepts_an_unapproved_plugin_without_minting_consent() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{"plugins":{"example":{"component":"provider.wasm"}}}"#,
    )
    .unwrap();

    let builder = load_test_builder(&config_path);
    let checks = builder.check_plugins().await.unwrap();

    assert_eq!(
        checks,
        [super::super::PluginCheck {
            instance_id: "example".to_owned(),
            required_environment_variables: Vec::new(),
        }]
    );
    assert!(!directory.join("consent.json").exists());
    let refusal = only_refusal(builder.start().await.err().unwrap());
    assert!(matches!(
        refusal.reason,
        PluginRefusalReason::ApprovalRequired
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn coherence_check_preserves_settings_schema_error_text() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component_with_schema(
            "example",
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","required":["api_key"]}"#,
        ),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{"plugins":{"example":{"component":"provider.wasm"}}}"#,
    )
    .unwrap();
    let builder = load_test_builder(&config_path);

    let check_refusal = only_refusal(builder.check_plugins().await.err().unwrap());
    let start_refusal = only_refusal(builder.start().await.err().unwrap());

    assert_eq!(check_refusal.to_string(), start_refusal.to_string());
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn coherence_check_reports_an_unset_required_environment_variable() {
    const REQUIRED_ENV: &str = "CHAP_TEST_COHERENCE_REQUIRED_UNSET_67DCD2BD";
    assert!(std::env::var_os(REQUIRED_ENV).is_none());
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component_requiring_env("example"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        format!(
            r#"{{"plugins":{{"example":{{"component":"provider.wasm","settings":{{"api_key_env":"{REQUIRED_ENV}"}}}}}}}}"#
        ),
    )
    .unwrap();

    let checks = load_test_builder(&config_path)
        .check_plugins()
        .await
        .unwrap();

    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].instance_id, "example");
    assert_eq!(checks[0].required_environment_variables.len(), 1);
    assert_eq!(
        checks[0].required_environment_variables[0].name,
        REQUIRED_ENV
    );
    assert!(!checks[0].required_environment_variables[0].present);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn admission_refusal_emits_a_warning_with_the_plugin_id() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "tracing-admission-test": { "component": "provider.wasm" }
            }
        }"#,
    )
    .unwrap();
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
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let error = match load_test_builder(&config_path).start().await {
        Ok(_) => panic!("unapproved plugin unexpectedly admitted"),
        Err(error) => error,
    };

    assert!(matches!(error, StartError::AdmissionRefused(_)));
    let output = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(output.contains("WARN"), "{output}");
    assert!(output.contains("plugin admission failed"), "{output}");
    assert!(output.contains("plugin=tracing-admission-test"), "{output}");
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn approving_then_denying_toggles_plugin_admission() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example": {
                    "component": "provider.wasm"
                }
            }
        }"#,
    )
    .unwrap();

    let builder = load_test_builder(&config_path);
    assert!(
        builder
            .review_plugin("example")
            .await
            .unwrap()
            .prior
            .is_none()
    );
    builder.approve_plugin("example").await.unwrap();
    let agent = builder.start().await.unwrap();
    tokio::task::spawn_blocking(move || drop(agent))
        .await
        .unwrap();

    let builder = load_test_builder(&config_path);
    builder.deny_plugin("example").unwrap();
    assert!(
        builder
            .review_plugin("example")
            .await
            .unwrap()
            .prior
            .is_none()
    );
    let error = builder.start().await.err().unwrap();
    let refusal = only_refusal(error);
    assert_eq!(refusal.instance_id, "example");
    assert!(matches!(
        refusal.reason,
        PluginRefusalReason::ApprovalRequired
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn start_refuses_a_plugin_that_gains_a_role_after_approval() {
    let (directory, component, builder) =
        role_change_plugin_builder(provider_component("example.plugin"));
    builder.approve_plugin("example").await.unwrap();
    fs::write(&component, provider_and_tool_component("example.plugin")).unwrap();

    let error = builder.start().await.err().unwrap();

    let refusal = only_refusal(error);
    assert_eq!(refusal.instance_id, "example");
    let PluginRefusalReason::RenewedApprovalRequired { drift } = refusal.reason else {
        panic!("expected renewed approval")
    };
    assert!(
        drift
            .export_changes
            .iter()
            .any(|change| change.kind == ExportDriftKind::Gained)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn losing_a_role_admits_and_refreshes_the_consent_record() {
    let (directory, component, builder) =
        role_change_plugin_builder(provider_and_tool_component("example.plugin"));
    builder.approve_plugin("example").await.unwrap();
    fs::write(&component, provider_component("example.plugin")).unwrap();
    let narrowed_exports = builder
        .review_plugin("example")
        .await
        .unwrap()
        .manifest
        .exported_interfaces;

    builder.start().await.unwrap();

    let records: std::collections::BTreeMap<String, ConsentRecord> =
        serde_json::from_slice(&fs::read(directory.join("consent.json")).unwrap()).unwrap();
    assert_eq!(records["example"].exported_interfaces, narrowed_exports);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn review_surfaces_a_gained_export_after_a_role_change() {
    let (directory, component, builder) =
        role_change_plugin_builder(provider_component("example.plugin"));
    builder.approve_plugin("example").await.unwrap();
    fs::write(&component, provider_and_tool_component("example.plugin")).unwrap();

    let review = builder.review_plugin("example").await.unwrap();
    let drift = review.drift.expect("the gained role should be reported");

    assert!(drift.export_changes.iter().any(|change| {
        change.name == "chap:agent/tools" && change.kind == ExportDriftKind::Gained
    }));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn nonblocking_drift_errors_are_reported_without_panicking() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(&component, provider_component("example.provider")).unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example": {
                    "component": "provider.wasm"
                }
            }
        }"#,
    )
    .unwrap();
    let builder = load_test_builder(&config_path);
    let manifest = builder.review_plugin("example").await.unwrap().manifest;

    let refusal = AgentBuilder::consent_refusal(
        "example",
        &component,
        ConsentRequired::Drift {
            manifest,
            drift: DriftReport {
                changes: Vec::new(),
                export_changes: Vec::new(),
                blocks_admission: false,
            },
        },
    );

    assert_eq!(refusal.instance_id, "example");
    assert_eq!(refusal.source_path, component);
    let PluginRefusalReason::RenewedApprovalRequired { drift } = refusal.reason else {
        panic!("expected renewed approval")
    };
    assert!(!drift.blocks_admission);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn rejects_missing_required_settings_during_prepare() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(
        &component,
        provider_component_with_schema(
            "example.provider",
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"model":{"type":"string"}},"required":["model"],"additionalProperties":false}"#,
        ),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example.provider": {
                    "component": "provider.wasm"
                }
            }
        }"#,
    )
    .unwrap();

    let error = match load_test_builder(&config_path).start().await {
        Ok(_) => panic!("missing required settings should be rejected"),
        Err(error) => error,
    };

    let refusal = only_refusal(error);
    assert_eq!(refusal.instance_id, "example.provider");
    assert_eq!(refusal.source_path, component);
    assert!(matches!(
        refusal.reason,
        PluginRefusalReason::ComponentLoad { source }
            if matches!(*source, ConsentError::LoadPlugin { .. })
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn validates_settings_before_loading_tool_definitions() {
    let directory = test_directory();
    let component = directory.join("tools.wasm");
    fs::write(
        &component,
        tool_component_with_schema(
            "example.tools",
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","required":["api_key"]}"#,
        ),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example.tools": {
                    "component": "tools.wasm"
                }
            }
        }"#,
    )
    .unwrap();

    let error = match load_test_builder(&config_path).start().await {
        Ok(_) => panic!("invalid tool settings should be rejected"),
        Err(error) => error,
    };

    let refusal = only_refusal(error);
    assert_eq!(refusal.instance_id, "example.tools");
    assert_eq!(refusal.source_path, component);
    assert!(matches!(
        refusal.reason,
        PluginRefusalReason::ComponentLoad { source }
            if matches!(*source, ConsentError::LoadPlugin { .. })
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn reports_framework_schema_transport_errors() {
    let directory = test_directory();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example": {
                    "component": "provider.wasm"
                }
            }
        }"#,
    )
    .unwrap();

    fs::write(
        directory.join("provider.wasm"),
        provider_component_with_trapping_schema("example"),
    )
    .unwrap();
    let error = match load_test_builder(&config_path).start().await {
        Ok(_) => panic!("a trapping schema export should be rejected"),
        Err(error) => error,
    };
    let refusal = only_refusal(error);
    assert_eq!(refusal.instance_id, "example");
    assert!(matches!(
        refusal.reason,
        PluginRefusalReason::ComponentLoad { source }
            if matches!(*source, ConsentError::LoadPlugin { .. })
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn discovers_a_configured_tool_plugin() {
    let directory = test_directory();
    let component = directory.join("tools.wasm");
    fs::write(&component, tool_component("example.tools")).unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example.tools": {
                    "component": "tools.wasm"
                }
            }
        }"#,
    )
    .unwrap();

    let builder = load_test_builder(&config_path);

    assert_eq!(builder.plugin_roles("example.tools").unwrap(), ["tool"]);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn loads_definitions_from_an_admitted_tool_plugin() {
    let directory = test_directory();
    fs::write(
        directory.join("tools.wasm"),
        tool_component("example.tools"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example.tools": {
                    "component": "tools.wasm"
                }
            }
        }"#,
    )
    .unwrap();

    let builder = load_test_builder(&config_path);
    builder.approve_plugin("example.tools").await.unwrap();
    let agent = builder.start().await.unwrap();

    assert_eq!(agent.tool_definitions()[0].name, "fixture-tool");
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn preserves_plugin_tool_registration_failures() {
    let directory = test_directory();
    fs::write(
        directory.join("tools.wasm"),
        tool_component("example.tools"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example.tools": {
                    "component": "tools.wasm"
                }
            }
        }"#,
    )
    .unwrap();
    let builder = load_test_builder(&config_path)
        .tool(NamedTool("fixture-tool"))
        .unwrap();
    builder.approve_plugin("example.tools").await.unwrap();

    let error = builder
        .start()
        .await
        .err()
        .expect("the duplicate plugin tool unexpectedly registered");

    assert_eq!(
        error.to_string(),
        "tool `fixture-tool` is already registered"
    );
    assert!(matches!(
        error,
        StartError::ToolRegistration(ToolRegistrationError::DuplicateName { name })
            if name == "fixture-tool"
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn times_out_a_hanging_tool_plugin() {
    let directory = test_directory();
    fs::write(
        directory.join("tools.wasm"),
        hanging_tool_component("example.tools"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "agent": {
                "budgets": {
                    "tools": { "deadline_ms": 1000 }
                }
            },
            "plugins": {
                "example.tools": {
                    "component": "tools.wasm"
                }
            }
        }"#,
    )
    .unwrap();

    let builder = load_test_builder(&config_path);
    builder.approve_plugin("example.tools").await.unwrap();
    let agent = builder.start().await.unwrap();

    let error = tokio::time::timeout(
        Duration::from_secs(5),
        agent.inner.tools.execute("fixture-tool", "{}".to_owned()),
    )
    .await
    .expect("hanging tool call did not respect its deadline")
    .unwrap_err();

    let ToolError::Failed(message) = error else {
        panic!("tool call deadline must be an execution failure")
    };
    assert!(
        message.starts_with("tool plugin `example.tools` timed out after "),
        "{message}"
    );
    assert!(!message.contains(" failed: "), "{message}");
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn loads_execution_modes_declared_by_an_admitted_tool_plugin() {
    let directory = test_directory();
    fs::write(
        directory.join("tools.wasm"),
        tool_component("example.tools"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example.tools": {
                    "component": "tools.wasm"
                }
            }
        }"#,
    )
    .unwrap();

    let builder = load_test_builder(&config_path);
    builder.approve_plugin("example.tools").await.unwrap();
    let agent = builder.start().await.unwrap();

    assert_eq!(
        agent.inner.tools.execution_mode("fixture-tool"),
        ExecutionMode::Parallel
    );
    assert_eq!(
        agent.inner.tools.execution_mode("sequential-tool"),
        ExecutionMode::Sequential
    );
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn plugin_execution_override_makes_loaded_tools_sequential() {
    let directory = test_directory();
    fs::write(
        directory.join("tools.wasm"),
        tool_component("example.tools"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "agent": {
                "tool_execution": {
                    "mode": "parallel"
                }
            },
            "plugins": {
                "example.tools": {
                    "component": "tools.wasm",
                    "tools": {
                        "execution": "sequential"
                    }
                }
            }
        }"#,
    )
    .unwrap();

    let builder = load_test_builder(&config_path);
    builder.approve_plugin("example.tools").await.unwrap();
    let agent = builder.start().await.unwrap();

    assert_eq!(
        agent.inner.tools.execution_mode("fixture-tool"),
        ExecutionMode::Sequential
    );
    assert_eq!(agent.inner.tool_execution.mode, ExecutionMode::Parallel);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn rejects_tools_config_for_a_provider_only_plugin() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example.provider": {
                    "component": "provider.wasm",
                    "tools": {
                        "execution": "sequential"
                    }
                }
            }
        }"#,
    )
    .unwrap();

    let error = load_test_builder(&config_path).start().await.err().unwrap();

    let refusal = only_refusal(error);
    assert_eq!(refusal.instance_id, "example.provider");
    assert!(matches!(
        refusal.reason,
        PluginRefusalReason::RoleConfigInvalid { ref role } if role == "tools"
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn rejects_context_config_for_a_provider_only_plugin() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example.provider": {
                    "component": "provider.wasm",
                    "context": {
                        "channel": "system"
                    }
                }
            }
        }"#,
    )
    .unwrap();

    let error = load_test_builder(&config_path).start().await.err().unwrap();

    let refusal = only_refusal(error);
    assert_eq!(refusal.instance_id, "example.provider");
    assert!(matches!(
        refusal.reason,
        PluginRefusalReason::RoleConfigInvalid { ref role } if role == "context"
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn accepts_an_instance_id_that_differs_from_plugin_metadata() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(&component, provider_component("embedded.id")).unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "config-id": {
                    "component": "provider.wasm"
                }
            }
        }"#,
    )
    .unwrap();

    let builder = load_test_builder(&config_path);
    builder.approve_plugin("config-id").await.unwrap();
    builder.start().await.unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn drops_partial_start_resources_on_a_blocking_thread() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "a-provider": {
                    "component": "provider.wasm"
                },
                "z-missing": {
                    "component": "missing.wasm"
                }
            }
        }"#,
    )
    .unwrap();
    let (dropped, observed_drop) = mpsc::sync_channel(1);
    let async_thread = std::thread::current().id();
    let builder = load_test_builder(&config_path)
        .tool(DropProbe { dropped })
        .unwrap();
    builder.approve_plugin("a-provider").await.unwrap();

    let error = match builder.start().await {
        Ok(_) => panic!("the missing second plugin should fail admission"),
        Err(error) => error,
    };

    let refusal = only_refusal(error);
    assert_eq!(refusal.instance_id, "z-missing");
    assert_eq!(refusal.source_path, directory.join("missing.wasm"));
    assert!(matches!(
        refusal.reason,
        PluginRefusalReason::ComponentLoad { source }
            if matches!(*source, ConsentError::ReadPlugin { .. })
    ));
    assert_ne!(observed_drop.recv().unwrap(), async_thread);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn reports_the_component_path_for_an_unsupported_plugin_role() {
    let (directory, component, builder) = unsupported_plugin_builder();
    let error = match builder.start().await {
        Ok(_) => panic!("a plugin without a supported role should be rejected"),
        Err(error) => error,
    };

    let refusal = only_refusal(error);
    assert_eq!(refusal.instance_id, "example");
    assert_eq!(refusal.source_path, component);
    assert!(matches!(
        refusal.reason,
        PluginRefusalReason::UnsupportedRole {
            ref exported_interfaces,
            ..
        } if exported_interfaces == &["lockgate:config/schema".to_owned()]
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn reviewing_rejects_a_plugin_without_a_supported_role() {
    let (directory, component, builder) = unsupported_plugin_builder();

    let error = match builder.review_plugin("example").await {
        Ok(_) => panic!("reviewing a plugin without a supported role should fail"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        ConsentError::UnsupportedRole {
            ref plugin,
            ref path,
            ref exported_interfaces,
            ..
        } if plugin == "example"
            && path == &component
            && exported_interfaces == &["lockgate:config/schema".to_owned()]
    ));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn approving_rejects_a_plugin_without_a_supported_role() {
    let (directory, component, builder) = unsupported_plugin_builder();

    let error = match builder.approve_plugin("example").await {
        Ok(_) => panic!("approving a plugin without a supported role should fail"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        ConsentError::UnsupportedRole {
            ref plugin,
            ref path,
            ref exported_interfaces,
            ..
        } if plugin == "example"
            && path == &component
            && exported_interfaces == &["lockgate:config/schema".to_owned()]
    ));
    fs::remove_dir_all(directory).unwrap();
}

fn unsupported_plugin_builder() -> (PathBuf, PathBuf, AgentBuilder) {
    let directory = test_directory();
    let component = directory.join("unsupported.wasm");
    fs::write(&component, unsupported_component("example.unsupported")).unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example": {
                    "component": "unsupported.wasm"
                }
            }
        }"#,
    )
    .unwrap();
    let builder = load_test_builder(&config_path);
    (directory, component, builder)
}

fn role_change_plugin_builder(bytes: Vec<u8>) -> (PathBuf, PathBuf, AgentBuilder) {
    let directory = test_directory();
    let component = directory.join("plugin.wasm");
    fs::write(&component, bytes).unwrap();
    let config_path = directory.join("chap.json");
    fs::write(
        &config_path,
        r#"{
            "plugins": {
                "example": {
                    "component": "plugin.wasm"
                }
            }
        }"#,
    )
    .unwrap();
    let builder = load_test_builder(&config_path);
    (directory, component, builder)
}

struct DropProbe {
    dropped: mpsc::SyncSender<std::thread::ThreadId>,
}

struct NamedTool(&'static str);

impl Tool for NamedTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.0.to_owned(),
            description: "A named test tool".to_owned(),
            parameters: r#"{"type":"object"}"#.to_owned(),
        }
    }

    fn execute(
        &self,
        _arguments: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, ToolError>> + Send + '_>>
    {
        Box::pin(async { Ok(String::new()) })
    }
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.dropped.send(std::thread::current().id()).unwrap();
    }
}

impl Tool for DropProbe {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "drop-probe".to_owned(),
            description: "Reports the thread that drops it".to_owned(),
            parameters: r#"{"type":"object"}"#.to_owned(),
        }
    }

    fn execute(
        &self,
        _arguments: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, ToolError>> + Send + '_>>
    {
        Box::pin(async { Ok(String::new()) })
    }
}

#[test]
fn derives_the_package_id_from_a_role_interface() {
    assert_eq!(
        super::super::role_package("chap:agent/provider@0.3.0"),
        "chap:agent@0.3.0"
    );
}

#[test]
fn derives_the_package_id_from_an_unversioned_interface() {
    assert_eq!(
        super::super::role_package("chap:agent/provider"),
        "chap:agent"
    );
}

#[test]
fn keeps_a_bare_export_name_as_the_package_id() {
    assert_eq!(super::super::role_package("settings"), "settings");
}

#[test]
fn resolves_supported_role_display_names_in_table_order() {
    let interfaces = vec![
        <super::super::bindings::context::Role as Role>::INTERFACE.to_owned(),
        <super::super::bindings::tools::Role as Role>::INTERFACE.to_owned(),
        <super::super::bindings::provider::Role as Role>::INTERFACE.to_owned(),
    ];

    assert_eq!(
        super::super::supported_roles(&interfaces),
        ["provider", "tool", "context"]
    );
}
