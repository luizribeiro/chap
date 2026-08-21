use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Field, Fields, Type};

pub(crate) fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            input.generics,
            "Settings cannot be derived for generic types",
        ));
    }

    let fields = match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(fields) => fields.named,
            fields => {
                return Err(syn::Error::new_spanned(
                    fields,
                    "Settings requires a struct with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new(
                input.ident.span(),
                "Settings requires a struct with named fields",
            ));
        }
    };

    let sdk = sdk_path(input.ident.span())?;
    let settings_name = input.ident;
    let proxy_name = format_ident!("__ChapSettingsProxyFor{settings_name}");

    let proxy_fields = fields
        .iter()
        .map(proxy_field)
        .collect::<syn::Result<Vec<_>>>()?;
    let field_names = fields
        .iter()
        .map(|field| field.ident.as_ref().expect("named fields were checked"));

    Ok(quote! {
        const _: () = {
            use #sdk as __chap;

            #[derive(__chap::serde::Deserialize, __chap::schemars::JsonSchema)]
            #[serde(
                crate = "__chap::serde",
                rename_all = "kebab-case",
                deny_unknown_fields
            )]
            #[schemars(crate = "__chap::schemars", rename_all = "kebab-case")]
            struct #proxy_name {
                #(#proxy_fields)*
            }

            #[automatically_derived]
            impl<'__chap_de> __chap::serde::Deserialize<'__chap_de> for #settings_name {
                fn deserialize<__ChapDeserializer>(
                    deserializer: __ChapDeserializer,
                ) -> ::core::result::Result<Self, __ChapDeserializer::Error>
                where
                    __ChapDeserializer: __chap::serde::Deserializer<'__chap_de>,
                {
                    let proxy = <#proxy_name as __chap::serde::Deserialize>::deserialize(
                        deserializer,
                    )?;
                    ::core::result::Result::Ok(Self {
                        #(#field_names: proxy.#field_names,)*
                    })
                }
            }

            #[automatically_derived]
            impl __chap::schemars::JsonSchema for #settings_name {
                fn inline_schema() -> bool {
                    <#proxy_name as __chap::schemars::JsonSchema>::inline_schema()
                }

                fn schema_name() -> __chap::alloc::borrow::Cow<'static, str> {
                    stringify!(#settings_name).into()
                }

                fn schema_id() -> __chap::alloc::borrow::Cow<'static, str> {
                    concat!(module_path!(), "::", stringify!(#settings_name)).into()
                }

                fn json_schema(
                    generator: &mut __chap::schemars::SchemaGenerator,
                ) -> __chap::schemars::Schema {
                    <#proxy_name as __chap::schemars::JsonSchema>::json_schema(generator)
                }
            }
        };
    })
}

fn proxy_field(field: &Field) -> syn::Result<TokenStream> {
    let name = field.ident.as_ref().expect("named fields were checked");
    let ty = &field.ty;
    let forwarded_attributes = field.attrs.iter().filter(|attribute| {
        ["doc", "serde", "schemars"]
            .iter()
            .any(|name| attribute.path().is_ident(name))
    });
    let non_empty =
        is_named_type(ty, "String").then(|| quote!(#[schemars(regex(pattern = r"\S"))]));

    Ok(quote! {
        #(#forwarded_attributes)*
        #non_empty
        #name: #ty,
    })
}

fn is_named_type(ty: &Type, expected: &str) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    path.qself.is_none()
        && path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == expected && segment.arguments.is_empty())
}

fn sdk_path(span: Span) -> syn::Result<TokenStream> {
    match crate_name("chap-plugin") {
        Ok(FoundCrate::Itself) => Ok(quote!(crate)),
        Ok(FoundCrate::Name(name)) => {
            let name = format_ident!("{name}");
            Ok(quote!(::#name))
        }
        Err(error) => Err(syn::Error::new(span, error.to_string())),
    }
}
