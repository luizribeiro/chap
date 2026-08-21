use super::{
    super::AgentBuilder,
    fixtures::{
        provider_component, provider_component_with_schema,
        provider_component_with_trapping_schema, test_directory, tool_component,
        tool_component_with_schema, unsupported_component,
    },
};
use crate::{ExecutionMode, SessionOptions, Tool, ToolDefinition};
use lockgate::{ConsentRequired, DriftReport, Role};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
};

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
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.provider"]
component = "provider.wasm"

[plugins."example.provider".settings]
model = "example-model"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();

    assert_eq!(builder.plugins().count(), 1);
    assert_eq!(
        builder.plugin_roles("example.provider").unwrap(),
        ["provider"]
    );
    let record = builder.approve_plugin("example.provider").await.unwrap();
    assert!(
        time::OffsetDateTime::parse(
            &record.approved_at,
            &time::format_description::well_known::Rfc3339
        )
        .is_ok()
    );
    let agent = builder.start().await.unwrap();
    assert!(agent.plugin_errors().next().is_none());
    assert!(agent.inner.plugins.contains_key("example.provider"));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn first_run_refuses_only_the_unapproved_plugin() {
    let directory = test_directory();
    fs::write(
        directory.join("provider.wasm"),
        provider_component("example.provider"),
    )
    .unwrap();
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins.approved]
component = "provider.wasm"

[plugins.unapproved]
component = "provider.wasm"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    builder.approve_plugin("approved").await.unwrap();
    let agent = builder.start().await.unwrap();
    let errors = agent.plugin_errors().collect::<Vec<_>>();

    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, "unapproved");
    assert!(errors[0].1.contains("requires approval"), "{}", errors[0].1);
    assert!(
        errors[0].1.contains("chap grants review unapproved"),
        "{}",
        errors[0].1
    );
    assert!(agent.inner.plugins.contains_key("approved"));
    assert!(!agent.inner.plugins.contains_key("unapproved"));
    assert_eq!(
        agent
            .session(SessionOptions::new("unapproved"))
            .err()
            .unwrap(),
        errors[0].1
    );
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
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins.example]
component = "provider.wasm"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
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
    assert!(agent.plugin_errors().next().is_none());
    tokio::task::spawn_blocking(move || drop(agent))
        .await
        .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    builder.deny_plugin("example").unwrap();
    assert!(
        builder
            .review_plugin("example")
            .await
            .unwrap()
            .prior
            .is_none()
    );
    let agent = builder.start().await.unwrap();
    assert!(
        agent
            .plugin_errors()
            .any(|(id, error)| id == "example" && error.contains("requires approval"))
    );
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn nonblocking_drift_errors_are_reported_without_panicking() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(&component, provider_component("example.provider")).unwrap();
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins.example]
component = "provider.wasm"
"#,
    )
    .unwrap();
    let builder = AgentBuilder::load(&config_path).unwrap();
    let manifest = builder.review_plugin("example").await.unwrap().manifest;

    let error = AgentBuilder::consent_error(
        "example",
        &component,
        ConsentRequired::Drift {
            manifest,
            drift: DriftReport {
                changes: Vec::new(),
                blocks_admission: false,
            },
        },
    );

    assert!(
        error.contains("reported a changed permission manifest"),
        "{error}"
    );
    assert!(error.contains("chap grants review example"), "{error}");
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
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.provider"]
component = "provider.wasm"
"#,
    )
    .unwrap();

    let error = match AgentBuilder::load(&config_path).unwrap().start().await {
        Ok(_) => panic!("missing required settings should be rejected"),
        Err(error) => error,
    };

    assert!(error.contains("settings"), "{error}");
    assert!(error.contains("model"), "{error}");
    assert!(error.contains("required"), "{error}");
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
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","required":["api-key"]}"#,
        ),
    )
    .unwrap();
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.tools"]
component = "tools.wasm"
"#,
    )
    .unwrap();

    let error = match AgentBuilder::load(&config_path).unwrap().start().await {
        Ok(_) => panic!("invalid tool settings should be rejected"),
        Err(error) => error,
    };

    assert!(error.contains("settings"), "{error}");
    assert!(error.contains("api-key"), "{error}");
    assert!(error.contains("required"), "{error}");
    assert!(!error.contains("tool plugin"));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn reports_framework_schema_transport_errors() {
    let directory = test_directory();
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins.example]
component = "provider.wasm"
"#,
    )
    .unwrap();

    fs::write(
        directory.join("provider.wasm"),
        provider_component_with_trapping_schema("example"),
    )
    .unwrap();
    let transport = match AgentBuilder::load(&config_path).unwrap().start().await {
        Ok(_) => panic!("a trapping schema export should be rejected"),
        Err(error) => error,
    };
    assert!(transport.contains("settings schema"), "{transport}");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn discovers_a_configured_tool_plugin() {
    let directory = test_directory();
    let component = directory.join("tools.wasm");
    fs::write(&component, tool_component("example.tools")).unwrap();
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.tools"]
component = "tools.wasm"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();

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
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.tools"]
component = "tools.wasm"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    builder.approve_plugin("example.tools").await.unwrap();
    let agent = builder.start().await.unwrap();

    assert!(agent.plugin_errors().next().is_none());
    assert_eq!(agent.tool_definitions()[0].name, "fixture-tool");
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
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.tools"]
component = "tools.wasm"
execution = "sequential"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    builder.approve_plugin("example.tools").await.unwrap();
    let agent = builder.start().await.unwrap();

    assert_eq!(
        agent.inner.tools.execution_mode("fixture-tool"),
        ExecutionMode::Sequential
    );
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn accepts_an_instance_id_that_differs_from_plugin_metadata() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(&component, provider_component("embedded.id")).unwrap();
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins.config-id]
component = "provider.wasm"
"#,
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    builder.approve_plugin("config-id").await.unwrap();
    let agent = builder.start().await.unwrap();
    assert!(agent.plugin_errors().next().is_none());
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
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins.a-provider]
component = "provider.wasm"

[plugins.z-missing]
component = "missing.wasm"
"#,
    )
    .unwrap();
    let (dropped, observed_drop) = mpsc::sync_channel(1);
    let async_thread = std::thread::current().id();
    let builder = AgentBuilder::load(&config_path)
        .unwrap()
        .tool(DropProbe { dropped })
        .unwrap();
    builder.approve_plugin("a-provider").await.unwrap();

    let error = match builder.start().await {
        Ok(_) => panic!("the missing second plugin should fail admission"),
        Err(error) => error,
    };

    assert!(error.contains("z-missing"), "{error}");
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

    assert_eq!(error, unsupported_role_error(&component));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn reviewing_rejects_a_plugin_without_a_supported_role() {
    let (directory, component, builder) = unsupported_plugin_builder();

    let error = match builder.review_plugin("example").await {
        Ok(_) => panic!("reviewing a plugin without a supported role should fail"),
        Err(error) => error,
    };

    assert_eq!(error, unsupported_role_error(&component));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn approving_rejects_a_plugin_without_a_supported_role() {
    let (directory, component, builder) = unsupported_plugin_builder();

    let error = match builder.approve_plugin("example").await {
        Ok(_) => panic!("approving a plugin without a supported role should fail"),
        Err(error) => error,
    };

    assert_eq!(error, unsupported_role_error(&component));
    fs::remove_dir_all(directory).unwrap();
}

fn unsupported_plugin_builder() -> (PathBuf, PathBuf, AgentBuilder) {
    let directory = test_directory();
    let component = directory.join("unsupported.wasm");
    fs::write(&component, unsupported_component("example.unsupported")).unwrap();
    let config_path = directory.join("chap.toml");
    fs::write(
        &config_path,
        r#"
[plugins.example]
component = "unsupported.wasm"
"#,
    )
    .unwrap();
    let builder = AgentBuilder::load(&config_path).unwrap();
    (directory, component, builder)
}

fn unsupported_role_error(component: &Path) -> String {
    format!(
        "plugin `example` from `{}` does not implement a supported role; expected an export from the `{}` package, but the component exports `lockgate:config/schema`",
        component.display(),
        super::super::role_package(<super::super::bindings::provider::Role as Role>::INTERFACE),
    )
}

struct DropProbe {
    dropped: mpsc::SyncSender<std::thread::ThreadId>,
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
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        Box::pin(async { Ok(String::new()) })
    }
}

#[test]
fn derives_the_package_id_from_a_role_interface() {
    assert_eq!(
        super::super::role_package("chap:agent/provider@0.2.0"),
        "chap:agent@0.2.0"
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
fn describes_an_export_list_with_backticked_names() {
    let interfaces = [
        <super::super::bindings::tools::Role as Role>::INTERFACE.to_owned(),
        "lockgate:config/schema".to_owned(),
    ];
    assert_eq!(
        super::super::describe_exports(&interfaces),
        format!(
            "`{}`, `lockgate:config/schema`",
            <super::super::bindings::tools::Role as Role>::INTERFACE
        )
    );
}

#[test]
fn describes_an_empty_export_list_as_no_interfaces() {
    assert_eq!(super::super::describe_exports(&[]), "no interfaces");
}
