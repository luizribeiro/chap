mod common;

use chap_core::{AgentBuilder, SessionOptions, install_crypto_provider};
use common::{HttpsMockServer, INVOCATION_TIMEOUT, build_openai_component};
use serde_json::json;
use std::path::Path;

#[tokio::test(flavor = "current_thread")]
async fn completes_a_provider_round_trip_over_tls() {
    install_crypto_provider();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let component = build_openai_component(&workspace);
    let mock = HttpsMockServer::start();
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("chap.json");
    std::fs::write(
        &config_path,
        json!({
            "plugins": {
                "openai": {
                    "component": component,
                    "settings": {
                        "base_url": format!("{}/v1", mock.origin),
                        "model": "mock-model",
                    },
                },
            },
        })
        .to_string(),
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path)
        .unwrap()
        .state_dir(directory.path())
        .tls_roots(mock.roots.clone());
    builder.approve_plugin(&"openai".into()).await.unwrap();
    let agent = builder.start().await.unwrap();
    let session = agent
        .session(SessionOptions::new("openai".into()))
        .await
        .unwrap();
    let completion = tokio::time::timeout(INVOCATION_TIMEOUT, session.send("hello over TLS"))
        .await
        .expect("TLS provider invocation timed out")
        .expect("TLS provider invocation failed");

    assert_eq!(completion, "done");
    let request = mock.finish();
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["model"], "mock-model");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "hello over TLS");
    assert_eq!(body["stream"], false);

    tokio::task::spawn_blocking(move || drop((session, agent)))
        .await
        .unwrap();
}
