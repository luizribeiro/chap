use chap_core::{AgentBuilder, CallBudget, PluginCall};
use lockgate::{HostBuilder, InvocationCtx, PluginConfig, PluginHandle, RuntimeLimits};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::Duration,
};

mod bindings {
    lockgate::host_bindings!({
        path: "../chap-wit/wit",
        world: "host",
    });
}

const INVOCATION_FUEL: u64 = 25_000_000;
const INVOCATION_DEADLINE: Duration = Duration::from_secs(30);

#[tokio::test]
async fn author_facing_sdk_plugins_admit_and_invoke_all_roles() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = sdk_components(&workspace);
    let config_directory = tempfile::tempdir().unwrap();
    let config_path = config_directory.path().join("chap.json");
    std::fs::write(
        &config_path,
        json!({
            "plugins": {
                "sdk-multi-role": {
                    "component": components.multi_role.display().to_string(),
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    let chap_builder = AgentBuilder::load(&config_path)
        .unwrap()
        .state_dir(config_directory.path());
    assert_eq!(
        chap_builder.plugin_roles("sdk-multi-role").unwrap(),
        ["provider", "tool", "context"]
    );
    let mut builder = HostBuilder::new(()).unwrap();
    let provider = admit(
        &mut builder,
        "sdk-provider-fixture",
        &components.provider,
        "single:",
    )
    .await;
    let multi_role = admit(
        &mut builder,
        "sdk-multi-role-fixture",
        &components.multi_role,
        "multi:",
    )
    .await;
    let context = admit_context(&mut builder, &components.context).await;
    let host = builder.finish();

    let completion = host
        .client::<bindings::provider::Role>(&provider)
        .unwrap()
        .complete(
            InvocationCtx::bounded(INVOCATION_FUEL, INVOCATION_DEADLINE),
            bindings::types::CompletionRequest {
                messages: vec![bindings::types::Message::User("hello".to_owned())],
                tools: Vec::new(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        completion.content.as_slice(),
        [bindings::types::AssistantContent::Text(text)] if text == "single:hello"
    ));
    assert!(matches!(
        completion.finish_reason,
        bindings::types::FinishReason::Stop
    ));

    let segments = host
        .client::<bindings::context::Role>(&context)
        .unwrap()
        .segments(InvocationCtx::bounded(INVOCATION_FUEL, INVOCATION_DEADLINE))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].id, "configured-context");
    assert_eq!(segments[0].content, "configured content");
    assert_eq!(segments[0].priority, -5);

    let first_completion = host
        .client::<bindings::provider::Role>(&multi_role)
        .unwrap()
        .complete(
            InvocationCtx::bounded(INVOCATION_FUEL, INVOCATION_DEADLINE),
            bindings::types::CompletionRequest {
                messages: vec![bindings::types::Message::User("use the tool".to_owned())],
                tools: Vec::new(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        first_completion.finish_reason,
        bindings::types::FinishReason::ToolCalls
    ));
    let call = match first_completion.content.as_slice() {
        [bindings::types::AssistantContent::ToolCall(call)] => call,
        content => panic!("expected one tool call, got {content:?}"),
    };
    assert_eq!(call.id, "fixture-call");
    assert_eq!(call.name, "sdk-echo");
    assert_eq!(call.arguments, r#"{"value":"round-trip"}"#);

    let tools = host.client::<bindings::tools::Role>(&multi_role).unwrap();
    let registrations = tools
        .definitions(InvocationCtx::bounded(INVOCATION_FUEL, INVOCATION_DEADLINE))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(registrations.len(), 1);
    assert_eq!(registrations[0].definition.name, call.name);
    assert!(matches!(
        registrations[0].execution_mode,
        bindings::types::ExecutionMode::Parallel
    ));
    let segments = host
        .client::<bindings::context::Role>(&multi_role)
        .unwrap()
        .segments(InvocationCtx::bounded(INVOCATION_FUEL, INVOCATION_DEADLINE))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].id, "sdk-context");
    assert_eq!(segments[0].content, "multi:context");
    assert_eq!(segments[0].priority, 7);
    let output = tools
        .execute(
            InvocationCtx::bounded(INVOCATION_FUEL, INVOCATION_DEADLINE),
            &call.name,
            &call.arguments,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output, r#"tool:{"value":"round-trip"}"#);

    let definitions = registrations
        .into_iter()
        .map(|registration| registration.definition)
        .collect();

    let final_completion = host
        .client::<bindings::provider::Role>(&multi_role)
        .unwrap()
        .complete(
            InvocationCtx::bounded(INVOCATION_FUEL, INVOCATION_DEADLINE),
            bindings::types::CompletionRequest {
                messages: vec![bindings::types::Message::ToolResult(
                    bindings::types::ToolResult {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        output,
                        is_error: false,
                    },
                )],
                tools: definitions,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        final_completion.content.as_slice(),
        [bindings::types::AssistantContent::Text(text)]
            if text == r#"multi:tool:{"value":"round-trip"}"#
    ));
    assert!(matches!(
        final_completion.finish_reason,
        bindings::types::FinishReason::Stop
    ));
}

#[tokio::test]
async fn sdk_tool_error_variants_cross_the_component_boundary() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = sdk_components(&workspace);
    let mut builder = HostBuilder::new(()).unwrap();
    let plugin = admit(
        &mut builder,
        "sdk-multi-role-fixture",
        &components.multi_role,
        "multi:",
    )
    .await;
    let host = builder.finish();
    let tools = host.client::<bindings::tools::Role>(&plugin).unwrap();
    let cases = [
        (
            "sdk-invalid-input",
            "arguments did not match the tool schema",
        ),
        ("sdk-denied", "capability policy denied the request"),
        ("sdk-failed", "remote tool execution returned an error"),
        ("sdk-fatal", "plugin runtime configuration is unavailable"),
    ];

    for (name, expected_message) in cases {
        let result = tools
            .execute(
                InvocationCtx::bounded(INVOCATION_FUEL, INVOCATION_DEADLINE),
                name,
                "{}",
            )
            .await
            .unwrap();
        let message = match (name, result) {
            ("sdk-invalid-input", Err(bindings::tools::ToolError::InvalidInput(message)))
            | ("sdk-denied", Err(bindings::tools::ToolError::Denied(message)))
            | ("sdk-failed", Err(bindings::tools::ToolError::Failed(message)))
            | ("sdk-fatal", Err(bindings::tools::ToolError::Fatal(message))) => message,
            _ => panic!("tool `{name}` returned the wrong wire error variant"),
        };
        assert_eq!(message, expected_message);
    }
}

#[tokio::test]
async fn times_out_a_tool_plugin_with_hanging_definitions() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = sdk_components(&workspace);
    let config_directory = tempfile::tempdir().unwrap();
    let config_path = config_directory.path().join("chap.json");
    std::fs::write(
        &config_path,
        json!({
            "plugins": {
                "sdk-multi-role": {
                    "component": components.multi_role.display().to_string(),
                    "settings": {
                        "prefix": "multi:",
                        "hang_definitions": true,
                    },
                },
            },
        })
        .to_string(),
    )
    .unwrap();
    let builder = AgentBuilder::load(&config_path)
        .unwrap()
        .state_dir(config_directory.path())
        .call_budget(
            PluginCall::ToolDefinitions,
            CallBudget {
                fuel: INVOCATION_FUEL,
                deadline: Duration::from_secs(1),
            },
        );
    builder.approve_plugin("sdk-multi-role").await.unwrap();

    // Generous outer bound: start() also wasmtime-compiles the component,
    // which loaded runners stretch far past the 1s deadline under test. A
    // true hang still trips this; only the deadline error proves the bound.
    let error = tokio::time::timeout(Duration::from_secs(60), builder.start())
        .await
        .expect("hanging tool definitions did not respect its deadline")
        .err()
        .expect("hanging tool definitions unexpectedly loaded");

    assert!(error.contains("tool plugin `sdk-multi-role`"), "{error}");
    assert!(
        error.contains("plugin exceeded its bounded call deadline of 1s"),
        "{error}"
    );
}

async fn admit(
    builder: &mut HostBuilder<()>,
    id: &str,
    component: &Path,
    prefix: &str,
) -> PluginHandle {
    let bytes = std::fs::read(component).unwrap();
    let prepared = builder
        .prepare(
            id,
            &bytes,
            PluginConfig {
                settings: Some(json!({ "prefix": prefix })),
                ..PluginConfig::default()
            },
        )
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(INVOCATION_FUEL, INVOCATION_DEADLINE),
        )
        .await
        .unwrap()
}

async fn admit_context(builder: &mut HostBuilder<()>, component: &Path) -> PluginHandle {
    let bytes = std::fs::read(component).unwrap();
    let prepared = builder
        .prepare(
            "sdk-context-fixture",
            &bytes,
            PluginConfig {
                settings: Some(json!({
                    "segments": [{
                        "id": "configured-context",
                        "content": "configured content",
                        "priority": -5,
                    }]
                })),
                ..PluginConfig::default()
            },
        )
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(INVOCATION_FUEL, INVOCATION_DEADLINE),
        )
        .await
        .unwrap()
}

struct Components {
    provider: PathBuf,
    multi_role: PathBuf,
    context: PathBuf,
}

fn sdk_components(workspace: &Path) -> &'static Components {
    static COMPONENTS: OnceLock<Components> = OnceLock::new();
    COMPONENTS.get_or_init(|| build_sdk_components(workspace))
}

fn build_sdk_components(workspace: &Path) -> Components {
    let output = Command::new(env!("CARGO"))
        .current_dir(workspace)
        .args([
            "build",
            "-p",
            "chap-sdk-provider-fixture",
            "-p",
            "chap-sdk-multi-role-fixture",
            "-p",
            "chap-sdk-context-fixture",
            "--target",
            "wasm32-wasip2",
        ])
        .output()
        .expect("run cargo to build SDK fixture components");
    assert!(
        output.status.success(),
        "failed to build SDK fixture components:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(target) if Path::new(&target).is_absolute() => PathBuf::from(target),
        Some(target) => workspace.join(target),
        None => workspace.join("target"),
    };
    let provider = target.join("wasm32-wasip2/debug/chap_sdk_provider_fixture.wasm");
    let multi_role = target.join("wasm32-wasip2/debug/chap_sdk_multi_role_fixture.wasm");
    let context = target.join("wasm32-wasip2/debug/chap_sdk_context_fixture.wasm");
    assert!(
        provider.is_file(),
        "provider fixture was not built at `{}`",
        provider.display()
    );
    assert!(
        multi_role.is_file(),
        "multi-role fixture was not built at `{}`",
        multi_role.display()
    );
    assert!(
        context.is_file(),
        "context fixture was not built at `{}`",
        context.display()
    );
    Components {
        provider,
        multi_role,
        context,
    }
}
