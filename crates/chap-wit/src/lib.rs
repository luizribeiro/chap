//! WIT sources and world composition for CHAP plugins.

const PACKAGE: &str = "package chap:agent@0.2.0;";
pub const WORLD: &str = "chap-plugin";
pub const TYPES_WIT: &str = include_str!("../wit/types.wit");

macro_rules! roles {
    ($(
        $name:ident = $rust_name:ident {
            interface: $interface:literal,
            display_name: $display_name:literal,
            wit: $wit:literal,
        }
    ),+ $(,)?) => {
        $(
            pub static $name: Role = Role {
                rust_name: stringify!($rust_name),
                interface: $interface,
                display_name: $display_name,
                wit: include_str!($wit),
            };
        )+

        pub static ROLES: &[&Role] = &[$(&$name),+];
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Role {
    pub rust_name: &'static str,
    pub interface: &'static str,
    pub display_name: &'static str,
    pub wit: &'static str,
}

#[cfg(feature = "exec")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Import {
    pub rust_name: &'static str,
    pub interface: &'static str,
    pub wit: &'static str,
}

roles! {
    PROVIDER = Provider {
        interface: "provider",
        display_name: "provider",
        wit: "../wit/provider.wit",
    },
    TOOLS = Tools {
        interface: "tools",
        display_name: "tool",
        wit: "../wit/tools.wit",
    },
    CONTEXT = Context {
        interface: "context",
        display_name: "context",
        wit: "../wit/context.wit",
    },
}

#[cfg(feature = "exec")]
pub static EXEC: Import = Import {
    rust_name: "Exec",
    interface: "exec",
    wit: include_str!("../wit/exec.wit"),
};

#[cfg(feature = "exec")]
pub static IMPORTS: &[&Import] = &[&EXEC];

pub fn resolve(name: &str) -> Option<&'static Role> {
    ROLES.iter().copied().find(|role| role.rust_name == name)
}

#[cfg(feature = "exec")]
pub fn resolve_import(name: &str) -> Option<&'static Import> {
    IMPORTS
        .iter()
        .copied()
        .find(|import| import.rust_name == name)
}

pub fn world(roles: &[&Role]) -> String {
    compose_world(roles, &[])
}

#[cfg(feature = "exec")]
pub fn world_with_imports(roles: &[&Role], imports: &[&Import]) -> String {
    let imports = imports
        .iter()
        .map(|import| (import.interface, import.wit))
        .collect::<Vec<_>>();
    compose_world(roles, &imports)
}

fn compose_world(roles: &[&Role], imports: &[(&str, &str)]) -> String {
    let mut wit = String::from(PACKAGE);
    wit.push_str(wit_body(TYPES_WIT));
    for (_, import_wit) in imports {
        wit.push_str(wit_body(import_wit));
    }
    for role in roles {
        wit.push_str(wit_body(role.wit));
    }
    wit.push_str("\nworld ");
    wit.push_str(WORLD);
    wit.push_str(" {\n  import types;\n");
    for (interface, _) in imports {
        wit.push_str("  import ");
        wit.push_str(interface);
        wit.push_str(";\n");
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeSet, path::Path};
    use wit_parser::{Resolve, WorldItem, WorldKey};

    #[test]
    fn role_table_is_self_consistent() {
        for role in ROLES {
            let resolved = resolve(role.rust_name).expect("every role must resolve by rust_name");
            assert!(std::ptr::eq(*role, resolved));
        }
    }

    #[test]
    fn host_world_exports_exactly_the_role_interfaces() {
        let mut resolve = Resolve::new();
        let (package, _) = resolve
            .push_path(Path::new(env!("CARGO_MANIFEST_DIR")).join("wit"))
            .unwrap();
        let host = resolve.packages[package].worlds["host"];
        let exports = resolve.worlds[host]
            .exports
            .iter()
            .map(|(key, item)| {
                let WorldItem::Interface { id, .. } = item else {
                    panic!("host world export `{key:?}` is not an interface");
                };
                match key {
                    WorldKey::Name(name) => name.clone(),
                    WorldKey::Interface(_) => resolve.interfaces[*id]
                        .name
                        .clone()
                        .expect("host world exports must be named"),
                }
            })
            .collect::<BTreeSet<_>>();
        let expected = std::iter::once("types")
            .chain(ROLES.iter().map(|role| role.interface))
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let missing = expected.difference(&exports).cloned().collect::<Vec<_>>();
        let unexpected = exports.difference(&expected).cloned().collect::<Vec<_>>();

        assert!(
            missing.is_empty() && unexpected.is_empty(),
            "host world exports disagree with chap_wit::ROLES; missing: [{}]; unexpected: [{}]",
            missing.join(", "),
            unexpected.join(", "),
        );
    }

    #[cfg(feature = "exec")]
    #[test]
    fn host_exec_world_imports_exec_and_exports_exactly_the_role_interfaces() {
        let mut resolve = Resolve::new();
        let (package, _) = resolve
            .push_path(Path::new(env!("CARGO_MANIFEST_DIR")).join("wit"))
            .unwrap();
        let host = resolve.packages[package].worlds["host-exec"];
        let imports = resolve.worlds[host]
            .imports
            .iter()
            .map(|(key, item)| {
                let WorldItem::Interface { id, .. } = item else {
                    panic!("host-exec world import `{key:?}` is not an interface");
                };
                match key {
                    WorldKey::Name(name) => name.clone(),
                    WorldKey::Interface(_) => resolve.interfaces[*id]
                        .name
                        .clone()
                        .expect("host-exec world imports must be named"),
                }
            })
            .collect::<BTreeSet<_>>();
        let exports = resolve.worlds[host]
            .exports
            .iter()
            .map(|(key, item)| {
                let WorldItem::Interface { id, .. } = item else {
                    panic!("host-exec world export `{key:?}` is not an interface");
                };
                match key {
                    WorldKey::Name(name) => name.clone(),
                    WorldKey::Interface(_) => resolve.interfaces[*id]
                        .name
                        .clone()
                        .expect("host-exec world exports must be named"),
                }
            })
            .collect::<BTreeSet<_>>();
        let expected_exports = std::iter::once("types")
            .chain(ROLES.iter().map(|role| role.interface))
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();

        assert_eq!(imports, BTreeSet::from([EXEC.interface.to_owned()]));
        assert_eq!(exports, expected_exports);
    }

    #[test]
    fn composes_a_single_role_world() {
        let expected = [
            PACKAGE,
            source_body(TYPES_WIT),
            source_body(PROVIDER.wit),
            "\nworld chap-plugin {\n  import types;\n  export provider;\n}\n",
        ]
        .concat();

        assert_eq!(world(&[&PROVIDER]), expected);
    }

    #[cfg(feature = "exec")]
    #[test]
    fn composes_a_single_role_world_with_an_exec_import() {
        let expected = [
            PACKAGE,
            source_body(TYPES_WIT),
            source_body(EXEC.wit),
            source_body(PROVIDER.wit),
            "\nworld chap-plugin {\n  import types;\n  import exec;\n  export provider;\n}\n",
        ]
        .concat();

        assert_eq!(world_with_imports(&[&PROVIDER], &[&EXEC]), expected);
    }

    #[test]
    fn composes_a_two_role_world() {
        let expected = [
            PACKAGE,
            source_body(TYPES_WIT),
            source_body(PROVIDER.wit),
            source_body(TOOLS.wit),
            "\nworld chap-plugin {\n  import types;\n  export provider;\n  export tools;\n}\n",
        ]
        .concat();

        assert_eq!(world(&[&PROVIDER, &TOOLS]), expected);
    }

    #[test]
    fn composes_a_three_role_world() {
        let expected = [
            PACKAGE,
            source_body(TYPES_WIT),
            source_body(PROVIDER.wit),
            source_body(TOOLS.wit),
            source_body(CONTEXT.wit),
            "\nworld chap-plugin {\n  import types;\n  export provider;\n  export tools;\n  export context;\n}\n",
        ]
        .concat();

        assert_eq!(world(&[&PROVIDER, &TOOLS, &CONTEXT]), expected);
    }

    fn source_body(source: &str) -> &str {
        source.split_once(';').unwrap().1
    }
}
