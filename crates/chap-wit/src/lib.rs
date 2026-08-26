//! WIT sources and world composition for CHAP plugins.

const PACKAGE: &str = "package chap:agent@0.2.0;";
pub const WORLD: &str = "chap-plugin";
pub const TYPES_WIT: &str = include_str!("../wit/types.wit");

macro_rules! roles {
    ($(
        $name:ident, $wit_name:ident = $rust_name:ident {
            interface: $interface:literal,
            display_name: $display_name:literal,
            wit: $wit:literal,
        }
    ),+ $(,)?) => {
        $(
            pub const $wit_name: &str = include_str!($wit);

            pub static $name: Role = Role {
                rust_name: stringify!($rust_name),
                interface: $interface,
                display_name: $display_name,
                wit: $wit_name,
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

roles! {
    PROVIDER, PROVIDER_WIT = Provider {
        interface: "provider",
        display_name: "provider",
        wit: "../wit/provider.wit",
    },
    TOOLS, TOOLS_WIT = Tools {
        interface: "tools",
        display_name: "tool",
        wit: "../wit/tools.wit",
    },
    CONTEXT, CONTEXT_WIT = Context {
        interface: "context",
        display_name: "context",
        wit: "../wit/context.wit",
    },
}

pub fn resolve(name: &str) -> Option<&'static Role> {
    ROLES.iter().copied().find(|role| role.rust_name == name)
}

pub fn world(roles: &[&Role]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_table_is_self_consistent() {
        for role in ROLES {
            let resolved = resolve(role.rust_name).expect("every role must resolve by rust_name");
            assert!(std::ptr::eq(*role, resolved));
        }
    }

    #[test]
    fn composes_a_single_role_world() {
        let expected = [
            PACKAGE,
            source_body(TYPES_WIT),
            source_body(PROVIDER_WIT),
            "\nworld chap-plugin {\n  import types;\n  export provider;\n}\n",
        ]
        .concat();

        assert_eq!(world(&[&PROVIDER]), expected);
    }

    #[test]
    fn composes_a_two_role_world() {
        let expected = [
            PACKAGE,
            source_body(TYPES_WIT),
            source_body(PROVIDER_WIT),
            source_body(TOOLS_WIT),
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
            source_body(PROVIDER_WIT),
            source_body(TOOLS_WIT),
            source_body(CONTEXT_WIT),
            "\nworld chap-plugin {\n  import types;\n  export provider;\n  export tools;\n  export context;\n}\n",
        ]
        .concat();

        assert_eq!(world(&[&PROVIDER, &TOOLS, &CONTEXT]), expected);
    }

    fn source_body(source: &str) -> &str {
        source.split_once(';').unwrap().1
    }
}
