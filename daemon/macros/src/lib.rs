use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

/// Derive a newtype helper impl set on a single-field tuple struct.
///
/// Currently emits a fixed kitchen-sink set of impls (`AsRef<str>`,
/// `Display`, and a UUID-generating `new()`). This will be replaced
/// with attribute-driven generation in a follow-up task.
#[proc_macro_derive(NewType, attributes(newtype))]
pub fn derive_new_type(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // Minimal validation up front; richer errors come in Task 3.
    if !matches!(&input.data, Data::Struct(s) if matches!(s.fields, Fields::Unnamed(ref f) if f.unnamed.len() == 1))
    {
        return syn::Error::new_spanned(
            &input.ident,
            "NewType requires a tuple struct with exactly one field",
        )
        .to_compile_error()
        .into();
    }

    quote! {
        impl #name {
            pub fn new() -> Self {
                Self(::uuid::Uuid::new_v4().to_string())
            }
        }
        impl ::core::convert::AsRef<str> for #name {
            #[inline]
            fn as_ref(&self) -> &str { &self.0 }
        }
        impl ::core::fmt::Display for #name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Display::fmt(&self.0, f)
            }
        }
    }
    .into()
}
