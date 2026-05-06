use darling::FromDeriveInput;
use proc_macro::TokenStream;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

mod codegen;
mod parse;

/// Derive helper impls on a single-field tuple struct based on
/// `#[newtype(...)]` attributes. See the spec for full attribute reference.
///
/// # Caller-side dependencies
///
/// - `serde` (with `derive` feature) when the call site uses `Serialize`/`Deserialize`
/// - `uuid` (with `v4` feature) when using `new(uuid_v4_string)` or `new(uuid_v4)`
///
/// `display` and `as_ref(T)` are not pre-validated; if the inner type does not
/// implement the required trait, rustc will emit a trait-resolution error
/// against the generated impl.
///
/// Manual `impl Default` or inherent `fn new()` collide with what this macro
/// emits; remove the corresponding `#[newtype(...)]` attribute when defining
/// them by hand.
#[proc_macro_derive(NewType, attributes(newtype))]
pub fn derive_new_type(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    if !input.generics.params.is_empty() || input.generics.where_clause.is_some() {
        return syn::Error::new_spanned(
            &input.generics,
            "NewType does not support generic parameters",
        )
        .to_compile_error()
        .into();
    }

    let data_struct = match &input.data {
        Data::Struct(s) => s,
        Data::Enum(_) | Data::Union(_) => {
            return syn::Error::new_spanned(
                &input.ident,
                "NewType can only be derived for structs",
            )
            .to_compile_error()
            .into();
        }
    };

    let unnamed = match &data_struct.fields {
        Fields::Unnamed(f) => f,
        Fields::Named(_) | Fields::Unit => {
            return syn::Error::new_spanned(
                &input.ident,
                "NewType requires a tuple struct",
            )
            .to_compile_error()
            .into();
        }
    };

    if unnamed.unnamed.len() != 1 {
        return syn::Error::new_spanned(
            &unnamed.unnamed,
            format!(
                "NewType requires exactly one field, found {}",
                unnamed.unnamed.len()
            ),
        )
        .to_compile_error()
        .into();
    }

    let attrs = match parse::NewTypeAttrs::from_derive_input(&input) {
        Ok(a) => a,
        Err(e) => return e.write_errors().into(),
    };
    if let Err(ts) = parse::validate(&attrs) {
        return ts.into();
    }

    codegen::generate(&input.ident, &attrs).into()
}
