use chap_core::AgentBuilder;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[tokio::test]
async fn migrated_kagi_component_admits_and_classifies_as_tools_only() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let component = build_kagi_component(&workspace);
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("chap.json");
    std::fs::write(
        &config_path,
        format!(
            r#"{{
                "plugins": {{
                    "kagi": {{
                        "component": {component:?},
                        "settings": {{
                            "api_key_env": "CHAP_TEST_KAGI_API_KEY"
                        }}
                    }}
                }}
            }}"#,
            component = component.display().to_string(),
        ),
    )
    .unwrap();

    let builder = AgentBuilder::load(&config_path).unwrap();
    assert_eq!(builder.plugin_roles("kagi").unwrap(), ["tool"]);
    builder.approve_plugin("kagi").await.unwrap();
    let agent = builder.start().await.unwrap();
    assert!(agent.plugin_errors().next().is_none());

    tokio::task::spawn_blocking(move || drop(agent))
        .await
        .unwrap();
}

fn build_kagi_component(workspace: &Path) -> PathBuf {
    let output = Command::new(env!("CARGO"))
        .current_dir(workspace)
        .args(["build", "-p", "chap-kagi", "--target", "wasm32-wasip2"])
        .output()
        .expect("run cargo to build the Kagi component");
    assert!(
        output.status.success(),
        "failed to build Kagi component:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(target) if Path::new(&target).is_absolute() => PathBuf::from(target),
        Some(target) => workspace.join(target),
        None => workspace.join("target"),
    };
    let component = target.join("wasm32-wasip2/debug/chap_kagi.wasm");
    assert!(
        component.is_file(),
        "Kagi component was not built at `{}`",
        component.display()
    );
    component
}
