use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

pub(crate) struct Role {
    pub(crate) wit: &'static chap_wit::Role,
    pub(crate) bridge: fn(&Ident) -> TokenStream,
    pub(crate) test_reference: fn(&Ident) -> TokenStream,
}

const ROLES: &[Role] = &[
    Role {
        wit: &chap_wit::PROVIDER,
        bridge: provider_bridge,
        test_reference: provider_test_reference,
    },
    Role {
        wit: &chap_wit::TOOLS,
        bridge: tools_bridge,
        test_reference: tools_test_reference,
    },
    Role {
        wit: &chap_wit::CONTEXT,
        bridge: context_bridge,
        test_reference: context_test_reference,
    },
];

pub(crate) fn resolve(name: &Ident) -> syn::Result<&'static Role> {
    let wit = chap_wit::resolve(&name.to_string()).ok_or_else(|| {
        syn::Error::new(name.span(), format!("unknown CHAP plugin role `{name}`"))
    })?;
    Ok(ROLES
        .iter()
        .find(|role| std::ptr::eq(role.wit, wit))
        .expect("every WIT role must have a code generator"))
}

fn provider_bridge(plugin: &Ident) -> TokenStream {
    quote! {
        #[automatically_derived]
        impl exports::chap::agent::provider::Guest for #plugin {
            async fn complete(
                request: ::chap_plugin::types::CompletionRequest,
            ) -> ::core::result::Result<
                ::chap_plugin::types::Completion,
                ::chap_plugin::types::ProviderError,
            > {
                let object = <#plugin as ::chap_plugin::Plugin>::new(
                    <#plugin as ::chap_plugin::__lockgate::Plugin>::settings(),
                );
                <#plugin as ::chap_plugin::provider::Provider>::complete(&object, request).await
            }
        }
    }
}

fn provider_test_reference(plugin: &Ident) -> TokenStream {
    quote! {
        let _ = <#plugin as ::chap_plugin::provider::Provider>::complete;
    }
}

fn tools_bridge(plugin: &Ident) -> TokenStream {
    quote! {
        #[automatically_derived]
        impl exports::chap::agent::tools::Guest for #plugin {
            fn definitions() -> ::core::result::Result<
                ::chap_plugin::alloc::vec::Vec<::chap_plugin::types::ToolRegistration>,
                ::chap_plugin::alloc::string::String,
            > {
                let object = <#plugin as ::chap_plugin::Plugin>::new(
                    <#plugin as ::chap_plugin::__lockgate::Plugin>::settings(),
                );
                let definitions = <#plugin as ::chap_plugin::tools::Tools>::definitions(&object)?;
                Ok(definitions
                    .into_iter()
                    .map(|definition| ::chap_plugin::types::ToolRegistration {
                        execution_mode: <#plugin as ::chap_plugin::tools::Tools>::execution_mode(
                            &object,
                            &definition.name,
                        ),
                        definition,
                    })
                    .collect())
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
                <#plugin as ::chap_plugin::tools::Tools>::execute(&object, name, arguments).await
            }
        }
    }
}

fn tools_test_reference(plugin: &Ident) -> TokenStream {
    quote! {
        let _ = <#plugin as ::chap_plugin::tools::Tools>::definitions;
        let _ = <#plugin as ::chap_plugin::tools::Tools>::execution_mode;
        let _ = <#plugin as ::chap_plugin::tools::Tools>::execute;
    }
}

fn context_bridge(plugin: &Ident) -> TokenStream {
    quote! {
        #[automatically_derived]
        impl exports::chap::agent::context::Guest for #plugin {
            async fn segments() -> ::core::result::Result<
                ::chap_plugin::alloc::vec::Vec<exports::chap::agent::context::Segment>,
                ::chap_plugin::alloc::string::String,
            > {
                let object = <#plugin as ::chap_plugin::Plugin>::new(
                    <#plugin as ::chap_plugin::__lockgate::Plugin>::settings(),
                );
                let segments =
                    <#plugin as ::chap_plugin::context::Context>::segments(&object).await?;
                Ok(segments
                    .into_iter()
                    .map(|segment| exports::chap::agent::context::Segment {
                        id: segment.id,
                        content: segment.content,
                        priority: segment.priority,
                    })
                    .collect())
            }
        }
    }
}

fn context_test_reference(plugin: &Ident) -> TokenStream {
    quote! {
        let _ = <#plugin as ::chap_plugin::context::Context>::segments;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wit_role_has_a_code_generator() {
        let missing = chap_wit::ROLES
            .iter()
            .copied()
            .filter(|wit| !ROLES.iter().any(|role| std::ptr::eq(role.wit, *wit)))
            .map(|wit| wit.rust_name)
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "roles missing a code generator in crates/chap-plugin-macros/src/roles.rs: {}",
            missing.join(", ")
        );
    }
}
