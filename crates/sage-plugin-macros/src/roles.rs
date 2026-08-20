use syn::Ident;

pub(crate) struct Role {
    pub(crate) rust_name: &'static str,
    pub(crate) interface: &'static str,
    pub(crate) wit: &'static str,
}

const PROVIDER_WIT: &str = include_str!("../../sage-plugin/wit/provider.wit");
const TOOLS_WIT: &str = include_str!("../../sage-plugin/wit/tools.wit");

const ROLES: &[Role] = &[
    Role {
        rust_name: "Provider",
        interface: "provider",
        wit: PROVIDER_WIT,
    },
    Role {
        rust_name: "Tools",
        interface: "tools",
        wit: TOOLS_WIT,
    },
];

pub(crate) fn resolve(name: &Ident) -> syn::Result<&'static Role> {
    ROLES
        .iter()
        .find(|role| name == role.rust_name)
        .ok_or_else(|| syn::Error::new(name.span(), format!("unknown SAGE plugin role `{name}`")))
}
