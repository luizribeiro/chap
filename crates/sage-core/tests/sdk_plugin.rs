use lockgate::{HostBuilder, InvocationCtx, PluginConfig, PluginHandle, RuntimeLimits};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

mod bindings {
    lockgate::host_bindings!({
        path: "../sage-plugin/wit",
        world: "host",
    });
}

const INVOCATION_FUEL: u64 = 25_000_000;

#[tokio::test]
async fn author_facing_sdk_plugins_admit_and_invoke_all_roles() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = build_sdk_components(&workspace);
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
    let host = builder.finish();

    let completion = host
        .client::<bindings::provider::Role>(&provider)
        .unwrap()
        .complete(
            InvocationCtx::bounded(INVOCATION_FUEL),
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

    let first_completion = host
        .client::<bindings::provider::Role>(&multi_role)
        .unwrap()
        .complete(
            InvocationCtx::bounded(INVOCATION_FUEL),
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
    let definitions = tools
        .definitions(InvocationCtx::bounded(INVOCATION_FUEL))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, call.name);
    let output = tools
        .execute(
            InvocationCtx::bounded(INVOCATION_FUEL),
            &call.name,
            &call.arguments,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output, r#"tool:{"value":"round-trip"}"#);

    let final_completion = host
        .client::<bindings::provider::Role>(&multi_role)
        .unwrap()
        .complete(
            InvocationCtx::bounded(INVOCATION_FUEL),
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
            InvocationCtx::bounded(INVOCATION_FUEL),
        )
        .await
        .unwrap()
}

struct Components {
    provider: PathBuf,
    multi_role: PathBuf,
}

fn build_sdk_components(workspace: &Path) -> Components {
    let output = Command::new(env!("CARGO"))
        .current_dir(workspace)
        .args([
            "build",
            "-p",
            "sage-sdk-provider-fixture",
            "-p",
            "sage-sdk-multi-role-fixture",
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
    let provider = target.join("wasm32-wasip2/debug/sage_sdk_provider_fixture.wasm");
    let multi_role = target.join("wasm32-wasip2/debug/sage_sdk_multi_role_fixture.wasm");
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
    Components {
        provider,
        multi_role,
    }
}
