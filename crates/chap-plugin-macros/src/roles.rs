use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

pub(crate) struct Role {
    pub(crate) rust_name: &'static str,
    pub(crate) interface: &'static str,
    pub(crate) wit: &'static str,
    pub(crate) bridge: fn(&Ident) -> TokenStream,
    pub(crate) test_reference: fn(&Ident) -> TokenStream,
}

const PROVIDER_WIT: &str = include_str!("../../chap-plugin/wit/provider.wit");
const TOOLS_WIT: &str = include_str!("../../chap-plugin/wit/tools.wit");

const ROLES: &[Role] = &[
    Role {
        rust_name: "Provider",
        interface: "provider",
        wit: PROVIDER_WIT,
        bridge: provider_bridge,
        test_reference: provider_test_reference,
    },
    Role {
        rust_name: "Tools",
        interface: "tools",
        wit: TOOLS_WIT,
        bridge: tools_bridge,
        test_reference: tools_test_reference,
    },
];

pub(crate) fn resolve(name: &Ident) -> syn::Result<&'static Role> {
    ROLES
        .iter()
        .find(|role| name == role.rust_name)
        .ok_or_else(|| syn::Error::new(name.span(), format!("unknown SAGE plugin role `{name}`")))
}

fn provider_bridge(plugin: &Ident) -> TokenStream {
    quote! {
        #[automatically_derived]
        impl exports::chap::agent::provider::Guest for #plugin {
            async fn complete(
                request: ::chap_plugin::types::CompletionRequest,
            ) -> ::core::result::Result<
                ::chap_plugin::types::Completion,
                ::chap_plugin::alloc::string::String,
            > {
                let object = <#plugin as ::chap_plugin::Plugin>::new(
                    <#plugin as ::chap_plugin::__lockgate::Plugin>::settings(),
                );
                <#plugin as ::chap_plugin::Provider>::complete(&object, request).await
            }
        }
    }
}

fn provider_test_reference(plugin: &Ident) -> TokenStream {
    quote! {
        let _ = <#plugin as ::chap_plugin::Provider>::complete;
    }
}

fn tools_bridge(plugin: &Ident) -> TokenStream {
    quote! {
        #[automatically_derived]
        impl exports::chap::agent::tools::Guest for #plugin {
            fn definitions() -> ::core::result::Result<
                ::chap_plugin::alloc::vec::Vec<::chap_plugin::types::ToolDefinition>,
                ::chap_plugin::alloc::string::String,
            > {
                let object = <#plugin as ::chap_plugin::Plugin>::new(
                    <#plugin as ::chap_plugin::__lockgate::Plugin>::settings(),
                );
                <#plugin as ::chap_plugin::Tools>::definitions(&object)
            }

            async fn execute(
                name: ::chap_plugin::alloc::string::String,
                arguments: ::chap_plugin::alloc::string::String,
            ) -> ::core::result::Result<
                ::chap_plugin::alloc::string::String,
                ::chap_plugin::alloc::string::String,
            > {
                let object = <#plugin as ::chap_plugin::Plugin>::new(
                    <#plugin as ::chap_plugin::__lockgate::Plugin>::settings(),
                );
                <#plugin as ::chap_plugin::Tools>::execute(&object, name, arguments).await
            }
        }
    }
}

fn tools_test_reference(plugin: &Ident) -> TokenStream {
    quote! {
        let _ = <#plugin as ::chap_plugin::Tools>::definitions;
        let _ = <#plugin as ::chap_plugin::Tools>::execute;
    }
}
