//! Procedural macros for the SAGE plugin SDK.

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

mod compose;
mod plugin;
mod roles;
mod settings;

/// Generates the component bindings for a selected set of SAGE roles.
#[proc_macro]
pub fn plugin(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as plugin::PluginInput);
    plugin::expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Applies SAGE's deserialization and JSON Schema settings conventions.
#[proc_macro_derive(Settings, attributes(settings))]
pub fn derive_settings(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    settings::expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
