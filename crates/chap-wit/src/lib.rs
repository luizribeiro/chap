//! WIT sources and world composition for CHAP plugins.

const PACKAGE: &str = "package chap:agent@0.2.0;";
pub const WORLD: &str = "chap-plugin";
pub const TYPES_WIT: &str = include_str!("../wit/types.wit");
pub const PROVIDER_WIT: &str = include_str!("../wit/provider.wit");
pub const TOOLS_WIT: &str = include_str!("../wit/tools.wit");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Role {
    pub rust_name: &'static str,
    pub interface: &'static str,
    pub display_name: &'static str,
    pub wit: &'static str,
}

pub static PROVIDER: Role = Role {
    rust_name: "Provider",
    interface: "provider",
    display_name: "provider",
    wit: PROVIDER_WIT,
};

pub static TOOLS: Role = Role {
    rust_name: "Tools",
    interface: "tools",
    display_name: "tool",
    wit: TOOLS_WIT,
};

pub static ROLES: &[&Role] = &[&PROVIDER, &TOOLS];

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

    fn source_body(source: &str) -> &str {
        source.split_once(';').unwrap().1
    }
}
