#![cfg(feature = "vm")]

mod common;

use chap_core::{AgentBuilder, SessionEvent, SessionOptions, ToolError};
use common::{
    INVOCATION_TIMEOUT, MockServer, ToolRequest, build_component, build_openai_component,
    provider_tool_output, receive_until_complete, tool_result,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

struct Components {
    sandbox: PathBuf,
    openai: PathBuf,
}

struct ScenarioResult {
    events: Vec<SessionEvent>,
    provider_output: String,
}

#[tokio::test]
async fn runs_a_command_in_a_vm_with_a_granted_mount() {
    let result = run_scenario(
        "granted",
        json!({"command": ["echo", "vm-e2e-ok"], "mounts": ["/project"]}),
    )
    .await;

    let output = tool_result(&result.events, "granted").as_ref().unwrap();
    assert!(output.contains("vm-e2e-ok"), "{output}");
    assert_eq!(result.provider_output, output.as_str());
}

#[tokio::test]
async fn denies_a_mount_outside_the_grant() {
    let result = run_scenario(
        "denied",
        json!({"command": ["echo", "x"], "mounts": ["/project", "/etc"]}),
    )
    .await;

    let ToolError::Denied(error) = tool_result(&result.events, "denied").as_ref().unwrap_err()
    else {
        panic!("an uncovered mount must remain a denied tool error")
    };
    assert!(error.contains("denied by the capability guard"), "{error}");
    assert!(error.contains("vm.mount"), "{error}");
    assert_eq!(result.provider_output, error.as_str());
}

async fn run_scenario(call_id: &str, arguments: Value) -> ScenarioResult {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = components(&workspace);
    let mock = MockServer::start(&[ToolRequest {
        id: call_id,
        name: "run",
        arguments,
    }]);
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("chap.json");
    write_config(&config_path, components, &mock.origin);

    let builder = AgentBuilder::load(&config_path)
        .unwrap()
        .state_dir(directory.path());
    builder.approve_plugin("sandbox").await.unwrap();
    builder.approve_plugin("openai").await.unwrap();
    let agent = builder.start().await.unwrap();
    let session = agent.session(SessionOptions::new("openai")).await.unwrap();
    let mut events = session.subscribe();
    let completion = tokio::time::timeout(INVOCATION_TIMEOUT, session.send("run the command"))
        .await
        .expect("agent invocation timed out")
        .expect("agent invocation failed");
    assert_eq!(completion, "done");
    let events = receive_until_complete(&mut events).await;
    let requests = mock.finish();
    let provider_output = provider_tool_output(&requests, call_id);

    tokio::task::spawn_blocking(move || drop((session, agent)))
        .await
        .unwrap();

    ScenarioResult {
        events,
        provider_output,
    }
}

fn write_config(path: &Path, components: &Components, origin: &str) {
    // Required setting references must resolve to a scope; the VM requests no egress.
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "plugins": {
                "sandbox": {
                    "component": components.sandbox.display().to_string(),
                    "settings": {
                        "image": "ghcr.io/acme/alpine:latest",
                        "allowed_mounts": ["/project"],
                        "allowed_egress": ["127.0.0.1:1"],
                    },
                    "tools": {
                        "execution": "sequential",
                    },
                },
                "openai": {
                    "component": components.openai.display().to_string(),
                    "settings": {
                        "base_url": format!("{origin}/v1"),
                        "model": "mock-model",
                    },
                },
            },
            "agent": {
                "vm": {
                    "registries": ["ghcr.io"],
                },
            },
        }))
        .unwrap(),
    )
    .unwrap();
}

fn components(workspace: &Path) -> &'static Components {
    static COMPONENTS: OnceLock<Components> = OnceLock::new();
    COMPONENTS.get_or_init(|| Components {
        sandbox: build_component(workspace, "chap-vm-plugin", "chap_vm_plugin.wasm"),
        openai: build_openai_component(workspace),
    })
}
