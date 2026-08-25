use super::super::{Agent, AgentBuilder, CONTEXT_ASSEMBLY_DEADLINE};
use crate::{SessionOptions, session::Message};
use serde_json::{Map, Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::Duration,
};

#[tokio::test]
async fn real_context_plugins_compose_for_the_request_but_not_history() {
    let (agent, _directory) = start_agent([
        (
            "later-context",
            json!({
                "segments": [{
                    "id": "later",
                    "content": "later guidance",
                    "priority": 20,
                }]
            }),
        ),
        (
            "earlier-context",
            json!({
                "segments": [{
                    "id": "earlier",
                    "content": "earlier guidance",
                    "priority": -10,
                }]
            }),
        ),
    ])
    .await;
    let session = agent
        .session(SessionOptions::new("fixture-provider"))
        .unwrap();

    assert_eq!(
        session.send("hello").await.unwrap(),
        "provider:earlier guidance\n\nlater guidance|hello"
    );
    let history = session.state.messages.read().await;
    assert!(
        !history
            .iter()
            .any(|message| matches!(message, Message::System(_))),
        "assembled context must never be stored in session history: {history:?}"
    );
}

#[tokio::test]
async fn hanging_real_context_plugin_hits_the_assembly_deadline() {
    let (agent, _directory) = start_agent([("hanging-context", json!({ "hang": true }))]).await;
    let session = agent
        .session(SessionOptions::new("fixture-provider"))
        .unwrap();

    let error = tokio::time::timeout(
        CONTEXT_ASSEMBLY_DEADLINE + Duration::from_secs(5),
        session.send("hello"),
    )
    .await
    .expect("context call did not respect its assembly deadline")
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("context plugin `hanging-context`"),
        "{error}"
    );
    assert!(error.to_string().contains("timed out after 10s"), "{error}");
}

async fn start_agent(
    contexts: impl IntoIterator<Item = (&'static str, Value)>,
) -> (Agent, tempfile::TempDir) {
    let components = sdk_components();
    let directory = tempfile::tempdir().unwrap();
    let mut plugins = Map::new();
    plugins.insert(
        "fixture-provider".to_owned(),
        json!({
            "component": components.provider.display().to_string(),
            "settings": { "prefix": "provider:" },
        }),
    );
    for (id, settings) in contexts {
        plugins.insert(
            id.to_owned(),
            json!({
                "component": components.context.display().to_string(),
                "settings": settings,
            }),
        );
    }
    let config_path = directory.path().join("chap.json");
    std::fs::write(&config_path, json!({ "plugins": plugins }).to_string()).unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    for id in plugins.keys() {
        builder.approve_plugin(id).await.unwrap();
    }
    (builder.start().await.unwrap(), directory)
}

struct Components {
    provider: PathBuf,
    context: PathBuf,
}

fn sdk_components() -> &'static Components {
    static COMPONENTS: OnceLock<Components> = OnceLock::new();
    COMPONENTS.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = Command::new(env!("CARGO"))
            .current_dir(&workspace)
            .args([
                "build",
                "-p",
                "chap-sdk-provider-fixture",
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
        let context = target.join("wasm32-wasip2/debug/chap_sdk_context_fixture.wasm");
        assert!(provider.is_file(), "missing `{}`", provider.display());
        assert!(context.is_file(), "missing `{}`", context.display());
        Components { provider, context }
    })
}
