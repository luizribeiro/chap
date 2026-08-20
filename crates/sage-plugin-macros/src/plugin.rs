use crate::{compose, roles};
use proc_macro2::TokenStream;
use quote::quote;
use std::collections::BTreeSet;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, LitStr, Token};

pub(crate) struct PluginInput {
    plugin: Ident,
    roles: Punctuated<Ident, Token![+]>,
}

impl Parse for PluginInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let plugin = input.parse()?;
        input.parse::<Token![:]>()?;
        let roles = Punctuated::parse_separated_nonempty(input)?;
        Ok(Self { plugin, roles })
    }
}

pub(crate) fn expand(input: PluginInput) -> syn::Result<TokenStream> {
    let mut seen = BTreeSet::new();
    let roles = input
        .roles
        .iter()
        .map(|name| {
            if !seen.insert(name.to_string()) {
                return Err(syn::Error::new(
                    name.span(),
                    format!("duplicate SAGE plugin role `{name}`"),
                ));
            }
            roles::resolve(name)
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let _plugin = input.plugin;
    let inline = LitStr::new(&compose::world(&roles), proc_macro2::Span::call_site());
    let world = LitStr::new(compose::WORLD, proc_macro2::Span::call_site());

    Ok(quote! {
        ::sage_plugin::generate!({
            inline: #inline,
            world: #world,
            facade: ::sage_plugin,
            with: {
                "sage:agent/types@0.2.0": ::sage_plugin::types,
            },
        });
    })
}
