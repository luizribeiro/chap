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

const PROVIDER_WIT: &str = include_str!("../../sage-plugin/wit/provider.wit");
const TOOLS_WIT: &str = include_str!("../../sage-plugin/wit/tools.wit");

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
        impl exports::sage::agent::provider::Guest for #plugin {
            async fn complete(
                request: ::sage_plugin::types::CompletionRequest,
            ) -> ::core::result::Result<
                ::sage_plugin::types::Completion,
                ::sage_plugin::alloc::string::String,
            > {
                let object = <#plugin as ::sage_plugin::Plugin>::new(
                    <#plugin as ::sage_plugin::__lockgate::Plugin>::settings(),
                );
                <#plugin as ::sage_plugin::Provider>::complete(&object, request).await
            }
        }
    }
}

fn provider_test_reference(plugin: &Ident) -> TokenStream {
    quote! {
        let _ = <#plugin as ::sage_plugin::Provider>::complete;
    }
}

fn tools_bridge(plugin: &Ident) -> TokenStream {
    quote! {
        #[automatically_derived]
        impl exports::sage::agent::tools::Guest for #plugin {
            fn definitions() -> ::core::result::Result<
                ::sage_plugin::alloc::vec::Vec<::sage_plugin::types::ToolDefinition>,
                ::sage_plugin::alloc::string::String,
            > {
                let object = <#plugin as ::sage_plugin::Plugin>::new(
                    <#plugin as ::sage_plugin::__lockgate::Plugin>::settings(),
                );
                <#plugin as ::sage_plugin::Tools>::definitions(&object)
            }

            async fn execute(
                name: ::sage_plugin::alloc::string::String,
                arguments: ::sage_plugin::alloc::string::String,
            ) -> ::core::result::Result<
                ::sage_plugin::alloc::string::String,
                ::sage_plugin::alloc::string::String,
            > {
                let object = <#plugin as ::sage_plugin::Plugin>::new(
                    <#plugin as ::sage_plugin::__lockgate::Plugin>::settings(),
                );
                <#plugin as ::sage_plugin::Tools>::execute(&object, name, arguments).await
            }
        }
    }
}

fn tools_test_reference(plugin: &Ident) -> TokenStream {
    quote! {
        let _ = <#plugin as ::sage_plugin::Tools>::definitions;
        let _ = <#plugin as ::sage_plugin::Tools>::execute;
    }
}
