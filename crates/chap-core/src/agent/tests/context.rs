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
async fn real_context_plugins_use_configured_channels_without_reaching_history() {
    let (agent, _directory) = start_agent([
        context_plugin(
            "later-context",
            json!({
                "segments": [{
                    "id": "later",
                    "content": "later guidance",
                    "priority": 20,
                }]
            }),
        ),
        context_plugin(
            "earlier-context",
            json!({
                "segments": [{
                    "id": "earlier",
                    "content": "earlier guidance",
                    "priority": -10,
                }]
            }),
        ),
        system_plugin(
            "later-system",
            json!({
                "segments": [{
                    "id": "later-system",
                    "content": "later operator guidance",
                    "priority": 40,
                }]
            }),
        ),
        system_plugin(
            "earlier-system",
            json!({
                "segments": [{
                    "id": "earlier-system",
                    "content": "earlier operator guidance",
                    "priority": -20,
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
        "provider:system:earlier operator guidance\n\nlater operator guidance|user:earlier guidance\n\nlater guidance|user:hello"
    );
    let history = session.state.messages.read().await;
    assert_eq!(history.len(), 2, "{history:?}");
    assert!(matches!(history.first(), Some(Message::User(input)) if input == "hello"));
    assert!(
        !history
            .iter()
            .any(|message| matches!(message, Message::System(_))),
        "assembled system context must never be stored in session history: {history:?}"
    );
}

#[tokio::test]
async fn hanging_real_context_plugin_hits_the_assembly_deadline() {
    let (agent, _directory) =
        start_agent([context_plugin("hanging-context", json!({ "hang": true }))]).await;
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
    contexts: impl IntoIterator<Item = ConfiguredContext>,
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
    for context in contexts {
        let mut configured = json!({
            "component": components.context.display().to_string(),
            "settings": context.settings,
        });
        if let Some(channel) = context.channel {
            configured["context"] = json!({ "channel": channel });
        }
        plugins.insert(context.id.to_owned(), configured);
    }
    let config_path = directory.path().join("chap.json");
    std::fs::write(&config_path, json!({ "plugins": plugins }).to_string()).unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    for id in plugins.keys() {
        builder.approve_plugin(id).await.unwrap();
    }
    (builder.start().await.unwrap(), directory)
}

struct ConfiguredContext {
    id: &'static str,
    channel: Option<&'static str>,
    settings: Value,
}

fn context_plugin(id: &'static str, settings: Value) -> ConfiguredContext {
    ConfiguredContext {
        id,
        channel: None,
        settings,
    }
}

fn system_plugin(id: &'static str, settings: Value) -> ConfiguredContext {
    ConfiguredContext {
        id,
        channel: Some("system"),
        settings,
    }
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
