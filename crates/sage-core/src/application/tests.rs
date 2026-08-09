use super::Application;
use crate::config::Config;
use std::{fs, path::Path, time::SystemTime};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

#[test]
fn loads_a_configured_provider() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(&component, provider_component("example.provider")).unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins."example.provider"]
component = "provider.wasm"

[plugins."example.provider".settings]
model = "example-model"
"#,
    )
    .unwrap();

    let config = Config::load(&config_path).unwrap();
    let application = Application::load(&config).unwrap();

    assert_eq!(application.plugin_count(), 1);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_a_config_id_that_differs_from_plugin_metadata() {
    let directory = test_directory();
    let component = directory.join("provider.wasm");
    fs::write(&component, provider_component("embedded.id")).unwrap();
    let config_path = directory.join("sage.toml");
    fs::write(
        &config_path,
        r#"
[plugins.config-id]
component = "provider.wasm"
"#,
    )
    .unwrap();

    let config = Config::load(&config_path).unwrap();
    let error = match Application::load(&config) {
        Ok(_) => panic!("mismatched plugin id should be rejected"),
        Err(error) => error,
    };

    assert!(error.contains("plugin `config-id` declares embedded id `embedded.id`"));
    fs::remove_dir_all(directory).unwrap();
}

fn provider_component(id: &str) -> Vec<u8> {
    let mut resolve = Resolve::new();
    let wit = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../wit");
    let package = resolve.push_path(wit).unwrap().0;
    let world = resolve.packages[package].worlds["provider-plugin"];
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    with_plugin_metadata(bytes, id)
}

fn with_plugin_metadata(mut bytes: Vec<u8>, id: &str) -> Vec<u8> {
    let metadata =
        format!(r#"{{"format":1,"id":"{id}","name":"Test provider","version":"0.1.0"}}"#);
    let section_name = "lockgate:plugin";
    let mut section = Vec::new();
    encode_u32(section_name.len() as u32, &mut section);
    section.extend_from_slice(section_name.as_bytes());
    section.extend_from_slice(metadata.as_bytes());
    bytes.push(0);
    encode_u32(section.len() as u32, &mut bytes);
    bytes.extend(section);
    bytes
}

fn encode_u32(mut value: u32, output: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            return;
        }
    }
}

fn test_directory() -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("sage-{}-{unique}", std::process::id()));
    fs::create_dir(&path).unwrap();
    path
}
