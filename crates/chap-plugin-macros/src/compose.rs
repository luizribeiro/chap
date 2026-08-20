use crate::roles::Role;

const TYPES_WIT: &str = include_str!("../../chap-plugin/wit/types.wit");
const PACKAGE: &str = "package chap:agent@0.2.0;";
pub(crate) const WORLD: &str = "chap-plugin";

pub(crate) fn world(roles: &[&Role]) -> String {
    let mut wit = String::from(PACKAGE);
    wit.push_str(wit_body(TYPES_WIT));
    for role in roles {
        wit.push_str(wit_body(role.wit));
    }
    wit.push_str("\nworld ");
    wit.push_str(WORLD);
    wit.push_str(" {\n  import types;\n");
    for role in roles {
        wit.push_str("  export ");
        wit.push_str(role.interface);
        wit.push_str(";\n");
    }
    wit.push_str("}\n");
    wit
}

fn wit_body(source: &str) -> &str {
    source.split_once(';').map_or(source, |(_, body)| body)
}
