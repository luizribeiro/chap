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
        arguments: json!({"command": "echo workspace-e2e-ok"}),
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
    assert!(
        chap_vm::host::MockVmBackend::recorded_commands()
            .iter()
            .any(|command| {
                command.args == ["sh", "-c", "echo workspace-e2e-ok"]
                    && command.cwd.as_deref() == Some("/mnt/workspace")
            })
    );
    let requests = mock.finish();
    assert_eq!(provider_tool_output(&requests, "workspace-call"), *output);

    tokio::task::spawn_blocking(move || drop((session, agent)))
        .await
        .unwrap();
}

#[tokio::test]
async fn exposes_sealed_secret_metadata_and_passes_the_spec_to_the_workspace_vm() {
    const HOST_ENV: &str = "CHAP_WORKSPACE_CAPABILITY_SECRET";
    const DUMMY_VALUE: &str = "workspace-capability-dummy-value";
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = components(&repository);
    let mock = MockServer::start(&[ToolRequest {
        id: "workspace-secret-call",
        name: "run",
        arguments: json!({"command": "true"}),
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
            "secrets": {
                "GITHUB_TOKEN": {
                    "from_env": HOST_ENV,
                    "hosts": ["api.github.com", "github.com"]
                }
            }
        })),
    );
    unsafe { std::env::set_var(HOST_ENV, DUMMY_VALUE) };
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
    let agent = builder.start().await.unwrap();
    let definition = agent
        .tool_definitions()
        .into_iter()
        .find(|definition| definition.name == "run")
        .unwrap();
    assert!(definition.description.contains("GITHUB_TOKEN"));
    assert!(
        definition
            .description
            .contains("api.github.com, github.com")
    );
    assert!(!definition.description.contains(HOST_ENV));
    assert!(!definition.description.contains(DUMMY_VALUE));

    let session = agent
        .session(SessionOptions::new("openai".parse().unwrap()))
        .await
        .unwrap();
    tokio::time::timeout(INVOCATION_TIMEOUT, session.send("run the command"))
        .await
        .expect("agent invocation timed out")
        .expect("agent invocation failed");
    assert!(chap_vm::host::MockVmBackend::recorded_secrets().contains(
        &chap_vm::host::SecretSpec {
            env: "GITHUB_TOKEN".into(),
            source: chap_vm::host::SecretSource::HostEnv(HOST_ENV.into()),
            hosts: vec!["api.github.com".into(), "github.com".into()],
        }
    ));
    assert!(
        !format!("{:?}", chap_vm::host::MockVmBackend::recorded_secrets()).contains(DUMMY_VALUE)
    );
    mock.finish();
    tokio::task::spawn_blocking(move || drop((session, agent)))
        .await
        .unwrap();
    unsafe { std::env::remove_var(HOST_ENV) };
}

#[tokio::test]
async fn refuses_to_start_when_a_workspace_secret_source_is_missing() {
    const HOST_ENV: &str = "CHAP_MISSING_WORKSPACE_CAPABILITY_SECRET";
    const UNRELATED_VALUE: &str = "must-not-appear";
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = components(&repository);
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("chap.json");
    write_config(
        &config_path,
        components,
        "http://127.0.0.1:1",
        Some(json!({
            "image": "docker.io/library/alpine:3.20",
            "mount": "ro",
            "secrets": {
                "GUEST_TOKEN": {
                    "from_env": HOST_ENV,
                    "hosts": ["api.example.com"]
                }
            }
        })),
    );
    unsafe { std::env::remove_var(HOST_ENV) };

    let error = AgentBuilder::load(&config_path)
        .unwrap()
        .state_dir(directory.path())
        .workspace_directory(directory.path())
        .start()
        .await
        .err()
        .expect("start must fail");
    let message = error.to_string();
    assert!(message.contains("GUEST_TOKEN"), "{message}");
    assert!(message.contains(HOST_ENV), "{message}");
    assert!(!message.contains(UNRELATED_VALUE), "{message}");
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

#[tokio::test]
async fn workspace_reports_the_lower_tools_or_exec_timeout() {
    for (tools_deadline_ms, exec_ceiling_ms, expected_ms) in
        [(20_000, 60_000, 20_000), (90_000, 40_000, 40_000)]
    {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("chap.json");
        std::fs::write(
            &config_path,
            serde_json::to_vec_pretty(&json!({
                "agent": {
                    "budgets": {"tools": {"deadline_ms": tools_deadline_ms}},
                    "vm": {
                        "registries": ["docker.io"],
                        "calls": {"exec_timeout_ceiling_ms": exec_ceiling_ms},
                    },
                    "workspace": {
                        "image": "docker.io/library/alpine:3.20",
                        "mount": "ro",
                    },
                },
            }))
            .unwrap(),
        )
        .unwrap();

        let agent = AgentBuilder::load(&config_path)
            .unwrap()
            .state_dir(directory.path())
            .workspace_directory(directory.path())
            .start()
            .await
            .unwrap();

        assert_eq!(agent.workspace().unwrap().exec_timeout_ms, expected_ms);
        tokio::task::spawn_blocking(move || drop(agent))
            .await
            .unwrap();
    }
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
