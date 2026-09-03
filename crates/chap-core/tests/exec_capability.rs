#![cfg(feature = "exec")]

mod common;

use chap_core::{AgentBuilder, SessionEvent, SessionOptions, ToolError};
use common::{
    INVOCATION_TIMEOUT, MockServer, ReceivedRequest, ToolRequest, build_component,
    build_openai_component, provider_tool_output, receive_until_complete, tool_result,
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, Instant},
};

struct Components {
    exec: PathBuf,
    openai: PathBuf,
}

struct ScenarioResult {
    events: Vec<SessionEvent>,
    requests: Vec<ReceivedRequest>,
    elapsed: Duration,
}

#[tokio::test]
async fn allowed_command_runs_through_the_guarded_import() {
    let result = run_scenario(
        &["echo"],
        &[ToolRequest {
            id: "allowed",
            name: "exec",
            arguments: json!({"program": "echo", "args": ["e2e-ok"]}),
        }],
    )
    .await;

    let output = tool_result(&result.events, "allowed").as_ref().unwrap();
    assert!(output.contains("Exit code: 0"), "{output}");
    assert!(output.contains("e2e-ok"), "{output}");
    assert_eq!(
        provider_tool_output(&result.requests, "allowed"),
        output.as_str()
    );
}

#[tokio::test]
async fn command_runs_from_the_invoking_directory() {
    let invoking_directory = std::env::current_dir().unwrap();
    let result = run_scenario(
        &["pwd"],
        &[ToolRequest {
            id: "working-directory",
            name: "exec",
            arguments: json!({"program": "pwd", "args": []}),
        }],
    )
    .await;

    let output = tool_result(&result.events, "working-directory")
        .as_ref()
        .unwrap();
    assert!(
        output
            .lines()
            .any(|line| line == invoking_directory.to_string_lossy()),
        "{output}"
    );
}

#[tokio::test]
async fn disallowed_command_is_denied_by_the_capability_guard() {
    let result = run_scenario(
        &["echo"],
        &[ToolRequest {
            id: "denied",
            name: "exec",
            arguments: json!({"program": "cat", "args": []}),
        }],
    )
    .await;

    let ToolError::Denied(error) = tool_result(&result.events, "denied").as_ref().unwrap_err()
    else {
        panic!("capability refusal must remain a denied tool error")
    };
    assert!(error.contains("denied by the capability guard"), "{error}");
    assert!(error.contains("exec.run"), "{error}");
    assert_eq!(
        provider_tool_output(&result.requests, "denied"),
        error.as_str()
    );
}

#[tokio::test]
async fn command_scopes_match_complete_argv_prefixes() {
    let result = run_scenario(
        &["echo hello"],
        &[
            ToolRequest {
                id: "matching-prefix",
                name: "exec",
                arguments: json!({"program": "echo", "args": ["hello"]}),
            },
            ToolRequest {
                id: "different-argument",
                name: "exec",
                arguments: json!({"program": "echo", "args": ["other"]}),
            },
        ],
    )
    .await;

    let output = tool_result(&result.events, "matching-prefix")
        .as_ref()
        .unwrap();
    assert!(output.contains("Exit code: 0"), "{output}");
    assert!(output.contains("hello"), "{output}");
    let ToolError::Denied(error) = tool_result(&result.events, "different-argument")
        .as_ref()
        .unwrap_err()
    else {
        panic!("capability refusal must remain a denied tool error")
    };
    assert!(error.contains("denied by the capability guard"), "{error}");
    assert!(error.contains("exec.run"), "{error}");
}

#[tokio::test]
async fn command_timeout_kills_the_process_at_the_deadline() {
    let result = run_scenario(
        &["sh"],
        &[ToolRequest {
            id: "timeout",
            name: "exec",
            arguments: json!({
                "program": "sh",
                "args": ["-c", "sleep 30"],
                "timeout_ms": 300,
            }),
        }],
    )
    .await;

    let ToolError::Failed(error) = tool_result(&result.events, "timeout").as_ref().unwrap_err()
    else {
        panic!("command timeout must remain a failed tool error")
    };
    assert!(
        error.ends_with("command timed out and was killed at the deadline"),
        "{error}"
    );
    assert!(
        result.elapsed < Duration::from_secs(5),
        "{:?}",
        result.elapsed
    );
}

async fn run_scenario(
    allowed_commands: &[&str],
    tool_requests: &[ToolRequest<'_>],
) -> ScenarioResult {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = components(&workspace);
    let mock = MockServer::start(tool_requests);
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("chap.json");
    write_config(&config_path, components, &mock.origin, allowed_commands);

    let builder = AgentBuilder::load(&config_path)
        .unwrap()
        .state_dir(directory.path());
    builder
        .approve_plugin(&"exec".parse().unwrap())
        .await
        .unwrap();
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
    let started = Instant::now();
    let completion = tokio::time::timeout(INVOCATION_TIMEOUT, session.send("run the command"))
        .await
        .expect("agent invocation timed out")
        .expect("agent invocation failed");
    let elapsed = started.elapsed();
    assert_eq!(completion, "done");
    let events = receive_until_complete(&mut events).await;
    let requests = mock.finish();

    tokio::task::spawn_blocking(move || drop((session, agent)))
        .await
        .unwrap();

    ScenarioResult {
        events,
        requests,
        elapsed,
    }
}

fn write_config(path: &Path, components: &Components, origin: &str, allowed_commands: &[&str]) {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "plugins": {
                "exec": {
                    "component": components.exec.display().to_string(),
                    "settings": {
                        "allowed_commands": allowed_commands,
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
        }))
        .unwrap(),
    )
    .unwrap();
}

fn components(workspace: &Path) -> &'static Components {
    static COMPONENTS: OnceLock<Components> = OnceLock::new();
    COMPONENTS.get_or_init(|| Components {
        exec: build_component(workspace, "chap-exec-plugin", "chap_exec_plugin.wasm"),
        openai: build_openai_component(workspace),
    })
}
