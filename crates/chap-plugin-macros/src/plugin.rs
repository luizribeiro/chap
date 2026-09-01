use crate::{imports, roles};
use proc_macro2::TokenStream;
use quote::quote;
use std::collections::BTreeSet;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, LitStr, Token};

mod kw {
    syn::custom_keyword!(imports);
}

pub(crate) struct PluginInput {
    plugin: Ident,
    roles: Punctuated<Ident, Token![+]>,
    imports: Punctuated<Ident, Token![+]>,
}

impl Parse for PluginInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let plugin = input.parse()?;
        input.parse::<Token![:]>()?;
        let mut roles = Punctuated::new();
        loop {
            roles.push_value(input.parse()?);
            if !input.peek(Token![+]) {
                break;
            }
            roles.push_punct(input.parse()?);
        }
        let imports = if input.is_empty() {
            Punctuated::new()
        } else {
            input.parse::<Token![,]>()?;
            input.parse::<kw::imports>()?;
            input.parse::<Token![:]>()?;
            Punctuated::parse_separated_nonempty(input)?
        };
        Ok(Self {
            plugin,
            roles,
            imports,
        })
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
                    format!("duplicate CHAP plugin role `{name}`"),
                ));
            }
            roles::resolve(name)
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let mut seen = BTreeSet::new();
    let imports = input
        .imports
        .iter()
        .map(|name| {
            if !seen.insert(name.to_string()) {
                return Err(syn::Error::new(
                    name.span(),
                    format!("duplicate CHAP plugin import `{name}`"),
                ));
            }
            imports::resolve(name)
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let plugin = input.plugin;
    let wit_roles = roles.iter().map(|role| role.wit).collect::<Vec<_>>();
    let wit_imports = imports.iter().map(|import| import.wit).collect::<Vec<_>>();
    let inline_wit = chap_wit::world(&wit_roles, &wit_imports);
    let inline = LitStr::new(&inline_wit, proc_macro2::Span::call_site());
    let world = LitStr::new(chap_wit::WORLD, proc_macro2::Span::call_site());
    let bridges = roles.iter().map(|role| (role.bridge)(&plugin));
    let test_references = roles.iter().map(|role| (role.test_reference)(&plugin));
    let with_mappings = imports.iter().map(|import| (import.with_mapping)());

    Ok(quote! {
        #[cfg(not(test))]
        ::chap_plugin::generate!({
            inline: #inline,
            world: #world,
            facade: ::chap_plugin,
            with: {
                "chap:agent/types@0.3.0": ::chap_plugin::types,
                #(#with_mappings)*
            },
        });

        #[automatically_derived]
        impl ::chap_plugin::__lockgate::Plugin for #plugin {
            const ID: &'static str = <Self as ::chap_plugin::Plugin>::ID;
            const DISPLAY_NAME: ::chap_plugin::MetadataSource =
                <Self as ::chap_plugin::Plugin>::DISPLAY_NAME;
            const VERSION: ::chap_plugin::MetadataSource =
                <Self as ::chap_plugin::Plugin>::VERSION;
            const DESCRIPTION: ::chap_plugin::MetadataSource =
                <Self as ::chap_plugin::Plugin>::DESCRIPTION;
            const LICENSE: ::chap_plugin::MetadataSource =
                <Self as ::chap_plugin::Plugin>::LICENSE;
            const REPOSITORY: ::chap_plugin::MetadataSource =
                <Self as ::chap_plugin::Plugin>::REPOSITORY;
            const HOMEPAGE: ::chap_plugin::MetadataSource =
                <Self as ::chap_plugin::Plugin>::HOMEPAGE;
            const NEEDS: ::chap_plugin::Needs = <Self as ::chap_plugin::Plugin>::NEEDS;
            const SETTINGS_POLICY: ::chap_plugin::SettingsPolicy =
                <Self as ::chap_plugin::Plugin>::SETTINGS_POLICY;
            type Settings = <Self as ::chap_plugin::Plugin>::Settings;
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
        ::chap_plugin::__lockgate::export!(
            #plugin;
            facade = ::chap_plugin::__lockgate
        );
    })
}

#[cfg(all(test, feature = "exec"))]
mod tests {
    use super::*;

    #[test]
    fn parses_an_import_axis() {
        let input = syn::parse_str::<PluginInput>("ExecTool: Tools, imports: Exec").unwrap();

        assert_eq!(input.roles.len(), 1);
        assert_eq!(input.roles[0], "Tools");
        assert_eq!(input.imports.len(), 1);
        assert_eq!(input.imports[0], "Exec");
    }

    #[test]
    fn rejects_duplicate_imports() {
        let input = syn::parse_str::<PluginInput>("ExecTool: Tools, imports: Exec + Exec").unwrap();
        let error = expand(input).unwrap_err();

        assert_eq!(error.to_string(), "duplicate CHAP plugin import `Exec`");
    }

    #[test]
    fn unknown_import_names_the_known_imports() {
        let input = syn::parse_str::<PluginInput>("ExecTool: Tools, imports: Other").unwrap();
        let error = expand(input).unwrap_err();

        assert_eq!(
            error.to_string(),
            "unknown CHAP plugin import `Other`; known imports: Exec"
        );
    }
}
