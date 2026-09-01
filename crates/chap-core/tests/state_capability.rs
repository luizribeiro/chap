#![cfg(feature = "state")]

mod common;

use chap_core::{AgentBuilder, SessionOptions};
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
    memory: PathBuf,
    openai: PathBuf,
}

#[tokio::test]
async fn state_persists_across_sequential_tool_calls() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = components(&workspace);
    let mock = MockServer::start(&[
        ToolRequest {
            id: "remember",
            name: "remember",
            arguments: json!({"key": "note", "value": "hello-from-e2e"}),
        },
        ToolRequest {
            id: "recall",
            name: "recall",
            arguments: json!({"key": "note"}),
        },
    ]);
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("chap.json");
    write_config(&config_path, components, &mock.origin);

    let builder = AgentBuilder::load(&config_path)
        .unwrap()
        .state_dir(directory.path());
    builder.approve_plugin("memory").await.unwrap();
    builder.approve_plugin("openai").await.unwrap();
    let agent = builder.start().await.unwrap();
    let session = agent.session(SessionOptions::new("openai")).await.unwrap();
    let mut events = session.subscribe();
    let completion = tokio::time::timeout(
        INVOCATION_TIMEOUT,
        session.send("remember the note, then recall it"),
    )
    .await
    .expect("agent invocation timed out")
    .expect("agent invocation failed");
    assert_eq!(completion, "done");
    let events = receive_until_complete(&mut events).await;
    let requests = mock.finish();

    tokio::task::spawn_blocking(move || drop((session, agent)))
        .await
        .unwrap();

    let remembered = tool_result(&events, "remember").as_ref().unwrap();
    assert!(remembered.contains("Stored"), "{remembered}");
    assert_eq!(
        provider_tool_output(&requests, "remember"),
        remembered.as_str()
    );
    let recalled = tool_result(&events, "recall").as_ref().unwrap();
    assert!(recalled.contains("hello-from-e2e"), "{recalled}");
    assert_eq!(provider_tool_output(&requests, "recall"), recalled.as_str());
}

fn write_config(path: &Path, components: &Components, origin: &str) {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "plugins": {
                "memory": {
                    "component": components.memory.display().to_string(),
                    "settings": {},
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
        }))
        .unwrap(),
    )
    .unwrap();
}

fn components(workspace: &Path) -> &'static Components {
    static COMPONENTS: OnceLock<Components> = OnceLock::new();
    COMPONENTS.get_or_init(|| Components {
        memory: build_component(workspace, "chap-state-plugin", "chap_state_plugin.wasm"),
        openai: build_openai_component(workspace),
    })
}
