use std::{fs, path::Path};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

pub(super) fn provider_component(id: &str) -> Vec<u8> {
    plugin_component(id, "provider-plugin", Some(permissive_schema()))
}

pub(super) fn provider_component_with_schema(id: &str, schema: &str) -> Vec<u8> {
    plugin_component(id, "provider-plugin", Some(schema))
}

pub(super) fn provider_component_with_trapping_schema(id: &str) -> Vec<u8> {
    plugin_component(id, "provider-plugin", None)
}

pub(super) fn tool_component(id: &str) -> Vec<u8> {
    plugin_component(id, "tool-plugin", Some(permissive_schema()))
}

pub(super) fn tool_component_with_schema(id: &str, schema: &str) -> Vec<u8> {
    plugin_component(id, "tool-plugin", Some(schema))
}

pub(super) fn unsupported_component(id: &str) -> Vec<u8> {
    let mut resolve = Resolve::new();
    resolve
        .push_str(
            "lockgate-config.wit",
            r#"
package lockgate:config;

interface schema {
  settings-schema: func() -> string;
}
"#,
        )
        .unwrap();
    let package = resolve
        .push_str(
            "fixture.wit",
            r#"
package chap:test;

world fixture {
  export lockgate:config/schema;
}
"#,
        )
        .unwrap();
    let world = resolve.packages[package].worlds["fixture"];
    let module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    let mut module = module_with_schema(&module, permissive_schema(), false);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    with_plugin_sections(bytes, id)
}

fn plugin_component(id: &str, world_name: &str, schema: Option<&str>) -> Vec<u8> {
    let mut resolve = Resolve::new();
    let wit = Path::new(env!("CARGO_MANIFEST_DIR")).join("../chap-plugin/wit");
    resolve.push_path(wit).unwrap();
    resolve
        .push_str(
            "lockgate-config.wit",
            r#"
package lockgate:config;

interface schema {
  settings-schema: func() -> string;
}
"#,
        )
        .unwrap();
    let wrapper = format!(
        r#"
package chap:test;

world fixture {{
  include chap:agent/{world_name}@0.2.0;
  export lockgate:config/schema;
}}
"#
    );
    let package = resolve.push_str("fixture.wit", &wrapper).unwrap();
    let world = resolve.packages[package].worlds["fixture"];
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    if let Some(schema) = schema {
        module = module_with_schema(&module, schema, world_name == "tool-plugin");
    }
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    with_plugin_sections(bytes, id)
}

fn module_with_schema(module: &[u8], schema: &str, tool_definitions: bool) -> Vec<u8> {
    let definitions_result = (16 + schema.len() + 3) & !3;
    let mut wat = wasmprinter::print_bytes(module).unwrap();
    wat = wat.replacen("(memory (;0;) 0)", "(memory (;0;) 1)", 1);
    wat = wat.replacen(
        "(func (;0;) (type 0) (result i32)\n    unreachable\n  )",
        "(func (;0;) (type 0) (result i32)\n    i32.const 0\n  )",
        1,
    );
    if tool_definitions {
        wat = wat.replacen(
            "(func (;2;) (type 0) (result i32)\n    unreachable\n  )",
            &format!("(func (;2;) (type 0) (result i32)\n    i32.const {definitions_result}\n  )"),
            1,
        );
    }
    let mut result = vec![16, 0, 0, 0];
    result.extend_from_slice(&(schema.len() as u32).to_le_bytes());
    let mut data = format!(
        "(data (i32.const 0) \"{}\")\n(data (i32.const 16) \"{}\")\n",
        wat_bytes(&result),
        wat_bytes(schema.as_bytes())
    );
    if tool_definitions {
        let parameters = r#"{"type":"object"}"#;
        let tools = [
            ("fixture-tool", "A fixture tool", 0_u8),
            ("sequential-tool", "A sequential fixture tool", 1_u8),
        ];
        let registrations = definitions_result + 12;
        let mut string_offset = registrations + tools.len() * 28;
        let mut registration_records = Vec::with_capacity(tools.len() * 28);
        let mut strings = Vec::new();

        for (name, description, execution_mode) in tools {
            for value in [name, description, parameters] {
                registration_records.extend_from_slice(&(string_offset as u32).to_le_bytes());
                registration_records.extend_from_slice(&(value.len() as u32).to_le_bytes());
                strings.push((string_offset, value));
                string_offset += value.len();
            }
            registration_records.extend_from_slice(&[execution_mode, 0, 0, 0]);
        }
        assert!(string_offset <= 65_536);

        let mut list_result = vec![0; 12];
        list_result[4..8].copy_from_slice(&(registrations as u32).to_le_bytes());
        list_result[8..12].copy_from_slice(&(tools.len() as u32).to_le_bytes());
        data.push_str(&format!(
            "(data (i32.const {definitions_result}) \"{}\")\n",
            wat_bytes(&list_result)
        ));
        data.push_str(&format!(
            "(data (i32.const {registrations}) \"{}\")\n",
            wat_bytes(&registration_records)
        ));
        for (offset, value) in strings {
            data.push_str(&format!(
                "(data (i32.const {offset}) \"{}\")\n",
                wat_bytes(value.as_bytes())
            ));
        }
    }
    data.push(')');
    wat.truncate(wat.strip_suffix(")\n").unwrap().len());
    wat.push_str(&data);
    wat::parse_str(wat).unwrap()
}

fn wat_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
}

fn permissive_schema() -> &'static str {
    r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object"}"#
}

fn with_plugin_sections(bytes: Vec<u8>, id: &str) -> Vec<u8> {
    let metadata =
        format!(r#"{{"format":1,"id":"{id}","name":"Test provider","version":"0.1.0"}}"#);
    let bytes = with_custom_section(bytes, "lockgate:plugin", metadata.as_bytes());
    with_custom_section(
        bytes,
        "lockgate:needs",
        br#"{"format":1,"optional":{},"reasons":{},"required":{}}"#,
    )
}

fn with_custom_section(mut bytes: Vec<u8>, section_name: &str, contents: &[u8]) -> Vec<u8> {
    let mut section = Vec::new();
    encode_u32(section_name.len() as u32, &mut section);
    section.extend_from_slice(section_name.as_bytes());
    section.extend_from_slice(contents);
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

pub(super) fn test_directory() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chap-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    fs::create_dir(&path).unwrap();
    path
}
