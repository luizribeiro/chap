//! Procedural macros for the SAGE plugin SDK.

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

mod settings;

/// Applies SAGE's deserialization and JSON Schema settings conventions.
#[proc_macro_derive(Settings, attributes(settings))]
pub fn derive_settings(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    settings::expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
