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
        &["/"],
        "///////////////////////",
        "/",
        json!({"command": ["echo", "vm-e2e-ok"]}),
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
        &["/", "/etc"],
        "/project///////////////",
        "/project",
        json!({"command": ["echo", "x"]}),
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

async fn run_scenario(
    call_id: &str,
    allowed_mounts: &[&str],
    encoded_mount_grant: &str,
    expected_mount_grant: &str,
    arguments: Value,
) -> ScenarioResult {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = components(&workspace);
    let mock = MockServer::start(&[ToolRequest {
        id: call_id,
        name: "run",
        arguments,
    }]);
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("chap.json");
    let sandbox =
        sandbox_with_mount_grant(&components.sandbox, directory.path(), encoded_mount_grant);
    write_config(
        &config_path,
        &sandbox,
        &components.openai,
        &mock.origin,
        allowed_mounts,
    );

    let builder = AgentBuilder::load(&config_path)
        .unwrap()
        .state_dir(directory.path());
    let sandbox_approval = builder.approve_plugin(&"sandbox".into()).await.unwrap();
    let mount_grant = sandbox_approval
        .grants
        .iter()
        .find(|grant| grant.capability == "vm" && grant.permission == "mount")
        .expect("the sandbox approval must include vm.mount");
    assert_eq!(mount_grant.scopes, [expected_mount_grant]);
    builder.approve_plugin(&"openai".into()).await.unwrap();
    let agent = builder.start().await.unwrap();
    let session = agent
        .session(SessionOptions::new("openai".into()))
        .await
        .unwrap();
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

fn sandbox_with_mount_grant(source: &Path, directory: &Path, grant_scope: &str) -> PathBuf {
    const NEEDS_SECTION: &[u8] = b"lockgate:needs";
    const SETTINGS_SCOPE: &[u8] = b"setting:/allowed_mounts";

    assert_eq!(SETTINGS_SCOPE.len(), grant_scope.len());
    let mut component = std::fs::read(source).unwrap();
    let needs_offset = component
        .windows(NEEDS_SECTION.len())
        .rposition(|window| window == NEEDS_SECTION)
        .expect("sandbox component must declare a needs section");
    let scope_offset = component[needs_offset..]
        .windows(SETTINGS_SCOPE.len())
        .position(|window| window == SETTINGS_SCOPE)
        .map(|offset| needs_offset + offset)
        .expect("sandbox needs must derive vm.mount from allowed_mounts");
    component[scope_offset..scope_offset + SETTINGS_SCOPE.len()]
        .copy_from_slice(grant_scope.as_bytes());

    let destination = directory.join("sandbox-fixed-mount-grant.wasm");
    std::fs::write(&destination, component).unwrap();
    destination
}

fn write_config(path: &Path, sandbox: &Path, openai: &Path, origin: &str, allowed_mounts: &[&str]) {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "plugins": {
                "sandbox": {
                    "component": sandbox.display().to_string(),
                    "settings": {
                        "image": "docker.io/library/alpine:3.20",
                        "allowed_mounts": allowed_mounts,
                        "allowed_egress": ["127.0.0.1:1"],
                    },
                    "tools": {
                        "execution": "sequential",
                    },
                },
                "openai": {
                    "component": openai.display().to_string(),
                    "settings": {
                        "base_url": format!("{origin}/v1"),
                        "model": "mock-model",
                    },
                },
            },
            "agent": {
                "vm": {
                    "registries": ["docker.io"],
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
