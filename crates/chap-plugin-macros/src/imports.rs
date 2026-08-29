use proc_macro2::TokenStream;
#[cfg(feature = "exec")]
use quote::quote;
use syn::Ident;

pub(crate) struct Import {
    pub(crate) rust_name: &'static str,
    #[cfg(feature = "exec")]
    pub(crate) wit: &'static chap_wit::Import,
    pub(crate) with_mapping: fn() -> TokenStream,
}

const IMPORTS: &[Import] = &[
    #[cfg(feature = "exec")]
    Import {
        rust_name: "Exec",
        wit: &chap_wit::EXEC,
        with_mapping: exec_with_mapping,
    },
];

pub(crate) fn resolve(name: &Ident) -> syn::Result<&'static Import> {
    IMPORTS
        .iter()
        .find(|import| name == import.rust_name)
        .ok_or_else(|| {
            let known = IMPORTS
                .iter()
                .map(|import| import.rust_name)
                .collect::<Vec<_>>()
                .join(", ");
            syn::Error::new(
                name.span(),
                format!("unknown CHAP plugin import `{name}`; known imports: {known}"),
            )
        })
}

#[cfg(feature = "exec")]
fn exec_with_mapping() -> TokenStream {
    quote! {
        "chap:agent/exec@0.2.0": ::chap_plugin::exec,
    }
}

#[cfg(all(test, feature = "exec"))]
mod tests {
    use super::*;

    #[test]
    fn every_wit_import_has_a_with_mapping() {
        let missing = chap_wit::IMPORTS
            .iter()
            .copied()
            .filter(|wit| !IMPORTS.iter().any(|import| std::ptr::eq(import.wit, *wit)))
            .map(|wit| wit.rust_name)
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "imports missing a with mapping in crates/chap-plugin-macros/src/imports.rs: {}",
            missing.join(", ")
        );
    }
}
