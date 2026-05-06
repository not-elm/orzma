use darling::{FromDeriveInput, FromMeta};
use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::Type;

/// Top-level `#[newtype(...)]` attributes on the derived struct.
#[derive(Debug, Default, FromDeriveInput)]
#[darling(default, attributes(newtype), forward_attrs(allow, doc, cfg))]
pub(crate) struct NewTypeAttrs {
    #[darling(multiple)]
    pub as_ref: Vec<AsRefArg>,
    pub display: Option<()>,
    pub new: Option<NewVariant>,
    pub default: Option<()>,
}

/// Wrapper around the `as_ref(T)` inner type. We accept any tokens
/// and parse them as `syn::Type` so primitives like `str` work.
#[derive(Debug)]
pub(crate) struct AsRefArg(pub Type);

impl FromMeta for AsRefArg {
    fn from_meta(item: &syn::Meta) -> darling::Result<Self> {
        match item {
            syn::Meta::List(list) => {
                let ty: Type = syn::parse2(list.tokens.clone())
                    .map_err(|e| darling::Error::custom(format!("invalid type for as_ref: {e}")))?;
                Ok(AsRefArg(ty))
            }
            _ => Err(darling::Error::unsupported_format(
                "as_ref requires a single type argument: `as_ref(T)`",
            )),
        }
    }
}

/// `new(uuid_v4_string)` / `new(uuid_v4)` / `new(default)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NewVariant {
    UuidV4String,
    UuidV4,
    Default,
}

impl FromMeta for NewVariant {
    fn from_meta(item: &syn::Meta) -> darling::Result<Self> {
        let syn::Meta::List(list) = item else {
            return Err(darling::Error::unsupported_format(
                "new requires one of: new(uuid_v4_string), new(uuid_v4), new(default)",
            ));
        };
        let ident: syn::Ident = syn::parse2(list.tokens.clone()).map_err(|_| {
            darling::Error::custom("new requires one of: uuid_v4_string, uuid_v4, default")
        })?;
        match ident.to_string().as_str() {
            "uuid_v4_string" => Ok(NewVariant::UuidV4String),
            "uuid_v4" => Ok(NewVariant::UuidV4),
            "default" => Ok(NewVariant::Default),
            other => Err(darling::Error::unknown_value(other)),
        }
    }
}

impl AsRefArg {
    /// Token-string used for duplicate detection.
    pub(crate) fn type_key(&self) -> String {
        self.0.to_token_stream().to_string()
    }
}

/// Cross-attribute validation: `default` requires `new(...)`, `as_ref` no dupes.
pub(crate) fn validate(attrs: &NewTypeAttrs) -> Result<(), TokenStream> {
    use std::collections::HashSet;

    if attrs.default.is_some() && attrs.new.is_none() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "default attribute requires new(...) to be specified",
        )
        .to_compile_error());
    }

    let mut seen: HashSet<String> = HashSet::new();
    for ar in &attrs.as_ref {
        if !seen.insert(ar.type_key()) {
            return Err(syn::Error::new_spanned(
                &ar.0,
                "as_ref(T) specified more than once for the same type",
            )
            .to_compile_error());
        }
    }

    Ok(())
}
