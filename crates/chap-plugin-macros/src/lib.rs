//! Procedural macros for the CHAP plugin SDK.

use proc_macro::TokenStream;
use syn::parse_macro_input;

mod compose;
mod plugin;
mod roles;

/// Generates the component bindings for a selected set of CHAP roles.
#[proc_macro]
pub fn plugin(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as plugin::PluginInput);
    plugin::expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
