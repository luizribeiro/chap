//! WIT sources and world composition for CHAP plugins.

const PACKAGE: &str = "package chap:agent@0.3.0;";
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

#[cfg(feature = "state")]
pub static STATE: Import = Import {
    rust_name: "State",
    interface: "state",
    wit: include_str!("../wit/state.wit"),
};

pub static IMPORTS: &[&Import] = &[
    #[cfg(feature = "exec")]
    &EXEC,
    #[cfg(feature = "state")]
    &STATE,
];

pub fn resolve(name: &str) -> Option<&'static Role> {
    ROLES.iter().copied().find(|role| role.rust_name == name)
}

pub fn resolve_import(name: &str) -> Option<&'static Import> {
    IMPORTS
        .iter()
        .copied()
        .find(|import| import.rust_name == name)
}

pub fn world(roles: &[&Role], imports: &[&Import]) -> String {
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

    const ALPHA: Role = Role {
        rust_name: "Alpha",
        interface: "alpha",
        display_name: "alpha",
        wit: "package chap:agent@0.3.0;\n\ninterface alpha {\n  run: func();\n}\n",
    };
    const GAMMA: Role = Role {
        rust_name: "Gamma",
        interface: "gamma",
        display_name: "gamma",
        wit: "package chap:agent@0.3.0;\n\ninterface gamma {\n  read: func() -> string;\n}\n",
    };
    const BETA: Import = Import {
        rust_name: "Beta",
        interface: "beta",
        wit: "package chap:agent@0.3.0;\n\ninterface beta {\n  write: func(value: string);\n}\n",
    };
    const DELTA: Import = Import {
        rust_name: "Delta",
        interface: "delta",
        wit: "package chap:agent@0.3.0;\n\ninterface delta {\n  reset: func();\n}\n",
    };

    #[test]
    fn role_table_is_self_consistent() {
        for role in ROLES {
            let resolved = resolve(role.rust_name).expect("every role must resolve by rust_name");
            assert!(std::ptr::eq(*role, resolved));
            assert_wit_entry(role.interface, role.wit);
        }
        for import in IMPORTS {
            let resolved =
                resolve_import(import.rust_name).expect("every import must resolve by rust_name");
            assert!(std::ptr::eq(*import, resolved));
            assert_wit_entry(import.interface, import.wit);
        }
    }

    #[test]
    fn wit_files_declare_the_composed_package_version() {
        let wit_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("wit");
        let mut checked = 0;
        for entry in std::fs::read_dir(&wit_dir).expect("the wit directory must be readable") {
            let path = entry
                .expect("every wit directory entry must be readable")
                .path();
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
            let declaration = source
                .lines()
                .find(|line| line.starts_with("package "))
                .unwrap_or_else(|| panic!("{} declares no package", path.display()));
            assert_eq!(
                declaration,
                PACKAGE,
                "{} disagrees with the composed package version",
                path.display()
            );
            checked += 1;
        }
        assert!(checked > 0, "no wit files were checked");
    }

    #[test]
    fn host_worlds_match_the_role_and_enabled_import_tables() {
        let mut resolve = Resolve::new();
        let (package, _) = resolve
            .push_path(Path::new(env!("CARGO_MANIFEST_DIR")).join("wit"))
            .unwrap();
        let expected_exports = std::iter::once("types")
            .chain(ROLES.iter().map(|role| role.interface))
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let enabled_imports = IMPORTS
            .iter()
            .map(|import| import.interface)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let mut checked = 0;

        for (name, host) in resolve.packages[package]
            .worlds
            .iter()
            .filter(|(name, _)| name.starts_with("host"))
        {
            let imports = world_interfaces(&resolve, name, &resolve.worlds[*host].imports);
            let exports = world_interfaces(&resolve, name, &resolve.worlds[*host].exports);
            assert_eq!(
                exports, expected_exports,
                "world `{name}` has wrong exports"
            );

            if imports.is_subset(&enabled_imports) {
                assert!(
                    imports
                        .iter()
                        .all(|interface| enabled_imports.contains(interface)),
                    "world `{name}` imports an interface absent from chap_wit::IMPORTS"
                );
                checked += 1;
            }
        }

        assert!(
            checked > 0,
            "no host worlds matched the enabled import table"
        );
    }

    #[test]
    fn composes_one_fixture_role_without_imports() {
        let expected = [
            PACKAGE,
            wit_body(TYPES_WIT),
            wit_body(ALPHA.wit),
            "\nworld chap-plugin {\n  import types;\n  export alpha;\n}\n",
        ]
        .concat();

        assert_eq!(world(&[&ALPHA], &[]), expected);
    }

    #[test]
    fn composes_one_fixture_role_with_one_import() {
        let expected = [
            PACKAGE,
            wit_body(TYPES_WIT),
            wit_body(BETA.wit),
            wit_body(ALPHA.wit),
            "\nworld chap-plugin {\n  import types;\n  import beta;\n  export alpha;\n}\n",
        ]
        .concat();

        assert_eq!(world(&[&ALPHA], &[&BETA]), expected);
    }

    #[test]
    fn composes_several_fixture_roles_and_imports() {
        let expected = [
            PACKAGE,
            wit_body(TYPES_WIT),
            wit_body(BETA.wit),
            wit_body(DELTA.wit),
            wit_body(ALPHA.wit),
            wit_body(GAMMA.wit),
            "\nworld chap-plugin {\n  import types;\n  import beta;\n  import delta;\n  export alpha;\n  export gamma;\n}\n",
        ]
        .concat();

        assert_eq!(world(&[&ALPHA, &GAMMA], &[&BETA, &DELTA]), expected);
    }

    fn assert_wit_entry(interface: &str, source: &str) {
        let declaration = source
            .lines()
            .find(|line| line.starts_with("package "))
            .unwrap_or_else(|| panic!("interface `{interface}` declares no package"));
        assert_eq!(
            declaration, PACKAGE,
            "interface `{interface}` disagrees with the composed package version"
        );

        let source = [PACKAGE, wit_body(TYPES_WIT), wit_body(source)].concat();
        let mut resolve = Resolve::new();
        let package = resolve
            .push_str(format!("{interface}.wit"), &source)
            .unwrap_or_else(|error| panic!("failed to parse interface `{interface}`: {error}"));
        assert!(
            resolve.packages[package].interfaces.contains_key(interface),
            "WIT for `{interface}` does not define that interface"
        );
    }

    fn world_interfaces<'a>(
        resolve: &Resolve,
        world: &str,
        items: impl IntoIterator<Item = (&'a WorldKey, &'a WorldItem)>,
    ) -> BTreeSet<String> {
        items
            .into_iter()
            .map(|(key, item)| {
                let WorldItem::Interface { id, .. } = item else {
                    panic!("world `{world}` item `{key:?}` is not an interface");
                };
                match key {
                    WorldKey::Name(name) => name.clone(),
                    WorldKey::Interface(_) => resolve.interfaces[*id]
                        .name
                        .clone()
                        .unwrap_or_else(|| panic!("world `{world}` interfaces must be named")),
                }
            })
            .collect()
    }
}
