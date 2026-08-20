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
    let plugin = input.plugin;
    let inline = LitStr::new(&compose::world(&roles), proc_macro2::Span::call_site());
    let world = LitStr::new(compose::WORLD, proc_macro2::Span::call_site());
    let bridges = roles.iter().map(|role| (role.bridge)(&plugin));
    let test_references = roles.iter().map(|role| (role.test_reference)(&plugin));

    Ok(quote! {
        #[cfg(not(test))]
        ::sage_plugin::generate!({
            inline: #inline,
            world: #world,
            facade: ::sage_plugin,
            with: {
                "chap:agent/types@0.2.0": ::sage_plugin::types,
            },
        });

        #[automatically_derived]
        impl ::sage_plugin::__lockgate::Plugin for #plugin {
            const ID: &'static str = <Self as ::sage_plugin::Plugin>::ID;
            const DISPLAY_NAME: ::sage_plugin::MetadataSource =
                <Self as ::sage_plugin::Plugin>::DISPLAY_NAME;
            const VERSION: ::sage_plugin::MetadataSource =
                <Self as ::sage_plugin::Plugin>::VERSION;
            const DESCRIPTION: ::sage_plugin::MetadataSource =
                <Self as ::sage_plugin::Plugin>::DESCRIPTION;
            const LICENSE: ::sage_plugin::MetadataSource =
                <Self as ::sage_plugin::Plugin>::LICENSE;
            const REPOSITORY: ::sage_plugin::MetadataSource =
                <Self as ::sage_plugin::Plugin>::REPOSITORY;
            const HOMEPAGE: ::sage_plugin::MetadataSource =
                <Self as ::sage_plugin::Plugin>::HOMEPAGE;
            const NEEDS: ::sage_plugin::Needs = <Self as ::sage_plugin::Plugin>::NEEDS;
            const SETTINGS_POLICY: ::sage_plugin::SettingsPolicy =
                <Self as ::sage_plugin::Plugin>::SETTINGS_POLICY;
            type Settings = <Self as ::sage_plugin::Plugin>::Settings;
        }

        #[cfg(not(test))]
        const _: () = {
            #(#bridges)*
        };

        #[cfg(test)]
        const _: () = {
            #(#test_references)*
        };

        #[cfg(not(test))]
        ::sage_plugin::__lockgate::export!(
            #plugin;
            facade = ::sage_plugin::__lockgate
        );
    })
}
