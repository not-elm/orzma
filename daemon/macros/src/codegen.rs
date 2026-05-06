use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

use crate::parse::{NewTypeAttrs, NewVariant};

pub(crate) fn generate(name: &Ident, attrs: &NewTypeAttrs) -> TokenStream {
    let mut out = TokenStream::new();

    for ar in &attrs.as_ref {
        let ty = &ar.0;
        out.extend(quote! {
            impl ::core::convert::AsRef<#ty> for #name {
                #[inline]
                fn as_ref(&self) -> &#ty {
                    ::core::convert::AsRef::<#ty>::as_ref(&self.0)
                }
            }
        });
    }

    if attrs.display.is_some() {
        out.extend(quote! {
            impl ::core::fmt::Display for #name {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    ::core::fmt::Display::fmt(&self.0, f)
                }
            }
        });
    }

    if let Some(variant) = attrs.new {
        let body = match variant {
            NewVariant::UuidV4String => quote! { Self(::uuid::Uuid::new_v4().to_string()) },
            NewVariant::UuidV4 => quote! { Self(::uuid::Uuid::new_v4()) },
            NewVariant::Default => quote! { Self(::core::default::Default::default()) },
        };
        out.extend(quote! {
            impl #name {
                pub fn new() -> Self { #body }
            }
        });
    }

    if attrs.default.is_some() {
        out.extend(quote! {
            impl ::core::default::Default for #name {
                fn default() -> Self { Self::new() }
            }
        });
    }

    out
}
