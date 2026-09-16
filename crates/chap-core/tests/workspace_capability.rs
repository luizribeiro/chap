#![cfg(feature = "vm")]

mod common;

use chap_core::{AgentBuilder, SessionEvent, SessionOptions, StartError};
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
    workspace: PathBuf,
    openai: PathBuf,
}

#[tokio::test]
async fn runs_a_command_in_the_host_owned_workspace() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = components(&repository);
    let mock = MockServer::start(&[ToolRequest {
        id: "workspace-call",
        name: "run",
        arguments: json!({"command": ["echo", "workspace-e2e-ok"]}),
    }]);
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("chap.json");
    write_config(
        &config_path,
        components,
        &mock.origin,
        Some(json!({
            "image": "docker.io/library/alpine:3.20",
            "mount": "ro",
            "egress": ["127.0.0.1:1"],
        })),
    );

    let builder = AgentBuilder::load(&config_path)
        .unwrap()
        .state_dir(directory.path())
        .workspace_directory(directory.path());
    let workspace_approval = builder
        .approve_plugin(&"workspace".parse().unwrap())
        .await
        .unwrap();
    let grants = workspace_approval
        .grants
        .iter()
        .filter(|grant| grant.capability == "vm")
        .collect::<Vec<_>>();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].permission, "manage");
    assert_eq!(grants[0].scopes, ["workspace"]);
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
    let events: Vec<SessionEvent> = receive_until_complete(&mut events).await;
    let output = tool_result(&events, "workspace-call").as_ref().unwrap();
    assert!(output.contains("echo workspace-e2e-ok"), "{output}");
    let requests = mock.finish();
    assert_eq!(provider_tool_output(&requests, "workspace-call"), *output);

    tokio::task::spawn_blocking(move || drop((session, agent)))
        .await
        .unwrap();
}

#[tokio::test]
async fn refuses_to_start_the_workspace_plugin_without_agent_workspace() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = components(&repository);
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("chap.json");
    write_config(&config_path, components, "http://127.0.0.1:1", None);
    let builder = AgentBuilder::load(&config_path)
        .unwrap()
        .state_dir(directory.path())
        .workspace_directory(directory.path());
    builder
        .approve_plugin(&"workspace".parse().unwrap())
        .await
        .unwrap();
    builder
        .approve_plugin(&"openai".parse().unwrap())
        .await
        .unwrap();

    let error = builder.start().await.err().expect("start must fail");

    assert!(matches!(
        &error,
        StartError::RoleReportedError {
            role: "tools",
            plugin_id,
            message,
        } if plugin_id.as_str() == "workspace" && message.contains("no workspace is configured")
    ));
}

fn write_config(path: &Path, components: &Components, origin: &str, workspace: Option<Value>) {
    let mut agent = json!({
        "vm": {"registries": ["docker.io"]},
    });
    if let Some(workspace) = workspace {
        agent["workspace"] = workspace;
    }
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "plugins": {
                "workspace": {
                    "component": components.workspace.display().to_string(),
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
            "agent": agent,
        }))
        .unwrap(),
    )
    .unwrap();
}

fn components(workspace: &Path) -> &'static Components {
    static COMPONENTS: OnceLock<Components> = OnceLock::new();
    COMPONENTS.get_or_init(|| Components {
        workspace: build_component(
            workspace,
            "chap-workspace-plugin",
            "chap_workspace_plugin.wasm",
        ),
        openai: build_openai_component(workspace),
    })
}
