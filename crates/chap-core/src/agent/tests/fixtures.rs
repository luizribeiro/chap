use std::{fs, path::Path};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

#[derive(Clone, Copy, Eq, PartialEq)]
enum RoleCall {
    Trap,
    Succeed,
    Hang,
}

pub(super) fn provider_component(id: &str) -> Vec<u8> {
    plugin_component(
        id,
        "provider-plugin",
        Some(permissive_schema()),
        RoleCall::Trap,
    )
}

pub(super) fn fast_provider_component(id: &str) -> Vec<u8> {
    plugin_component(
        id,
        "provider-plugin",
        Some(permissive_schema()),
        RoleCall::Succeed,
    )
}

pub(super) fn hanging_provider_component(id: &str) -> Vec<u8> {
    plugin_component(
        id,
        "provider-plugin",
        Some(permissive_schema()),
        RoleCall::Hang,
    )
}

pub(super) fn provider_component_with_schema(id: &str, schema: &str) -> Vec<u8> {
    plugin_component(id, "provider-plugin", Some(schema), RoleCall::Trap)
}

pub(super) fn provider_component_with_trapping_schema(id: &str) -> Vec<u8> {
    plugin_component(id, "provider-plugin", None, RoleCall::Trap)
}

pub(super) fn tool_component(id: &str) -> Vec<u8> {
    plugin_component(id, "tool-plugin", Some(permissive_schema()), RoleCall::Trap)
}

pub(super) fn fast_tool_component(id: &str) -> Vec<u8> {
    plugin_component(
        id,
        "tool-plugin",
        Some(permissive_schema()),
        RoleCall::Succeed,
    )
}

pub(super) fn hanging_tool_component(id: &str) -> Vec<u8> {
    plugin_component(id, "tool-plugin", Some(permissive_schema()), RoleCall::Hang)
}

pub(super) fn tool_component_with_schema(id: &str, schema: &str) -> Vec<u8> {
    plugin_component(id, "tool-plugin", Some(schema), RoleCall::Trap)
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
    let mut module = module_with_schema(&module, permissive_schema(), false, false, RoleCall::Trap);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    with_plugin_sections(bytes, id)
}

fn plugin_component(
    id: &str,
    world_name: &str,
    schema: Option<&str>,
    role_call: RoleCall,
) -> Vec<u8> {
    let mut resolve = Resolve::new();
    let wit = Path::new(env!("CARGO_MANIFEST_DIR")).join("../chap-plugin/wit");
    resolve.push_path(wit).unwrap();
    let clock_import = if role_call == RoleCall::Hang {
        resolve
            .push_str(
                "io.wit",
                r#"
package wasi:io@0.2.12;

interface poll {
  resource pollable {
    block: func();
  }
}
"#,
            )
            .unwrap();
        resolve
            .push_str(
                "clocks.wit",
                r#"
package wasi:clocks@0.2.12;

interface monotonic-clock {
  use wasi:io/poll@0.2.12.{pollable};
  type duration = u64;
  subscribe-duration: func(when: duration) -> pollable;
}
"#,
            )
            .unwrap();
        "import wasi:clocks/monotonic-clock@0.2.12;\n  import wasi:io/poll@0.2.12;"
    } else {
        ""
    };
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
  {clock_import}
  include chap:agent/{world_name}@0.2.0;
  export lockgate:config/schema;
}}
"#
    );
    let package = resolve.push_str("fixture.wit", &wrapper).unwrap();
    let world = resolve.packages[package].worlds["fixture"];
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    if let Some(schema) = schema {
        module = module_with_schema(
            &module,
            schema,
            true,
            world_name == "tool-plugin",
            role_call,
        );
    }
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    with_plugin_sections(bytes, id)
}

fn module_with_schema(
    module: &[u8],
    schema: &str,
    role_export: bool,
    tool_definitions: bool,
    role_call: RoleCall,
) -> Vec<u8> {
    const ROLE_RESULT: usize = 4096;

    let has_clock_import = role_call == RoleCall::Hang;
    let function_offset = if has_clock_import { 3 } else { 0 };
    let schema_function_type = if has_clock_import { 2 } else { 0 };
    let role_function_type = if has_clock_import { 3 } else { 2 };
    let definitions_result = (16 + schema.len() + 3) & !3;
    let mut wat = wasmprinter::print_bytes(module).unwrap();
    wat = wat.replacen("(memory (;0;) 0)", "(memory (;0;) 1)", 1);
    replace_trapping_function(
        &mut wat,
        function_offset,
        schema_function_type,
        "(result i32)",
        "i32.const 0",
    );
    if tool_definitions {
        replace_trapping_function(
            &mut wat,
            2 + function_offset,
            schema_function_type,
            "(result i32)",
            &format!("i32.const {definitions_result}"),
        );
    }
    if role_export {
        let realloc_function = if tool_definitions { 6 } else { 4 } + function_offset;
        replace_trapping_function(
            &mut wat,
            realloc_function,
            role_function_type,
            "(param i32 i32 i32 i32) (result i32)",
            "i32.const 8192",
        );
        let role_function = if tool_definitions { 4 } else { 2 } + function_offset;
        let role_signature = "(param i32 i32 i32 i32) (result i32)";
        match role_call {
            RoleCall::Trap => {}
            RoleCall::Succeed => replace_trapping_function(
                &mut wat,
                role_function,
                role_function_type,
                role_signature,
                &format!("i32.const {ROLE_RESULT}"),
            ),
            RoleCall::Hang => replace_trapping_function(
                &mut wat,
                role_function,
                role_function_type,
                role_signature,
                "i64.const 3600000000000\n    call 2\n    call 0\n    unreachable",
            ),
        }
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
    if role_call == RoleCall::Succeed {
        let result_size = if tool_definitions { 12 } else { 104 };
        data.push_str(&format!(
            "(data (i32.const {ROLE_RESULT}) \"{}\")\n",
            wat_bytes(&vec![0; result_size])
        ));
    }
    data.push(')');
    wat.truncate(wat.strip_suffix(")\n").unwrap().len());
    wat.push_str(&data);
    wat::parse_str(wat).unwrap()
}

fn replace_trapping_function(
    wat: &mut String,
    function: usize,
    function_type: usize,
    signature: &str,
    body: &str,
) {
    let before =
        format!("(func (;{function};) (type {function_type}) {signature}\n    unreachable\n  )");
    let after = format!("(func (;{function};) (type {function_type}) {signature}\n    {body}\n  )");
    assert!(wat.contains(&before), "missing fixture function `{before}`");
    *wat = wat.replacen(&before, &after, 1);
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
