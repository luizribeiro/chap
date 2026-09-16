#![cfg(feature = "vm")]

mod common;

use chap_core::{AgentBuilder, SessionEvent, SessionOptions, ToolError};
use common::{
    INVOCATION_TIMEOUT, MockServer, ToolRequest, build_component, build_openai_component,
    provider_tool_output, receive_until_complete, tool_result,
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

struct Components {
    vm_guest: PathBuf,
    openai: PathBuf,
}

struct ScenarioResult {
    events: Vec<SessionEvent>,
    provider_output: String,
}

#[tokio::test]
async fn permits_a_read_write_mount_within_the_grant() {
    let directory = tempfile::tempdir().unwrap();
    let allowed = directory.path().join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let allowed = allowed.canonicalize().unwrap();
    let mount = format!("rw:{}", allowed.display());

    let result = run_scenario("allowed", directory.path(), &mount, &mount).await;

    let output = tool_result(&result.events, "allowed").as_ref().unwrap();
    assert!(output.contains("echo vm-e2e-ok"), "{output}");
    assert_eq!(result.provider_output, output.as_str());
}

#[tokio::test]
async fn denies_a_mount_outside_the_grant() {
    let directory = tempfile::tempdir().unwrap();
    let allowed = directory.path().join("allowed");
    let other = directory.path().join("other");
    std::fs::create_dir(&allowed).unwrap();
    std::fs::create_dir(&other).unwrap();
    let allowed = allowed.canonicalize().unwrap();
    let other = other.canonicalize().unwrap();
    let grant = format!("rw:{}", allowed.display());
    let requested = format!("rw:{}", other.display());

    let result = run_scenario("outside", directory.path(), &grant, &requested).await;

    assert_denied(&result, "outside");
}

#[tokio::test]
async fn denies_read_write_when_only_read_only_is_granted() {
    let directory = tempfile::tempdir().unwrap();
    let allowed = directory.path().join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let allowed = allowed.canonicalize().unwrap();
    let grant = format!("ro:{}", allowed.display());
    let requested = format!("rw:{}", allowed.display());

    let result = run_scenario("readonly", directory.path(), &grant, &requested).await;

    assert_denied(&result, "readonly");
}

fn assert_denied(result: &ScenarioResult, call_id: &str) {
    let ToolError::Denied(error) = tool_result(&result.events, call_id).as_ref().unwrap_err()
    else {
        panic!("an uncovered mount must remain a denied tool error")
    };
    assert!(error.contains("denied by the capability guard"), "{error}");
    assert!(error.contains("vm.mount"), "{error}");
    assert_eq!(result.provider_output, error.as_str());
}

async fn run_scenario(
    call_id: &str,
    state_dir: &Path,
    allowed_mount: &str,
    requested_mount: &str,
) -> ScenarioResult {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = components(&repository);
    let mock = MockServer::start(&[ToolRequest {
        id: call_id,
        name: "run",
        arguments: json!({
            "mount": requested_mount,
            "command": ["echo", "vm-e2e-ok"],
        }),
    }]);
    let config_path = state_dir.join("chap.json");
    write_config(&config_path, components, &mock.origin, &[allowed_mount]);

    let builder = AgentBuilder::load(&config_path)
        .unwrap()
        .state_dir(state_dir);
    let approval = builder
        .approve_plugin(&"vm-guest".parse().unwrap())
        .await
        .unwrap();
    let mount_grant = approval
        .grants
        .iter()
        .find(|grant| grant.capability == "vm" && grant.permission == "mount")
        .expect("the vm guest approval must include vm.mount");
    assert_eq!(mount_grant.scopes, [allowed_mount]);
    builder
        .approve_plugin(&"openai".parse().unwrap())
        .await
        .unwrap();

    let agent = builder.start().await.unwrap();
    let session = agent
        .session(SessionOptions::new("openai".parse().unwrap()))
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

fn write_config(path: &Path, components: &Components, origin: &str, allowed_mounts: &[&str]) {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "plugins": {
                "vm-guest": {
                    "component": components.vm_guest.display().to_string(),
                    "settings": {
                        "allowed_mounts": allowed_mounts,
                        "allowed_egress": ["127.0.0.1:1"],
                    },
                    "tools": {"execution": "sequential"},
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
                "vm": {"registries": ["docker.io"]},
            },
        }))
        .unwrap(),
    )
    .unwrap();
}

fn components(workspace: &Path) -> &'static Components {
    static COMPONENTS: OnceLock<Components> = OnceLock::new();
    COMPONENTS.get_or_init(|| Components {
        vm_guest: build_component(
            workspace,
            "chap-vm-guest-fixture",
            "chap_vm_guest_fixture.wasm",
        ),
        openai: build_openai_component(workspace),
    })
}
