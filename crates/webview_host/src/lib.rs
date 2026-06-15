//! Tokio-free host integration for ozmux: a per-handle runtime root for the
//! webview control plane and (behind the `cef` feature) serving
//! dynamically-registered Tier 1 webview assets through an `ozma-dyn://`
//! custom scheme via `DynAssetRegistry`.

pub mod asset;
pub mod dyn_scheme;
pub mod host;

#[cfg(feature = "cef")]
pub use dyn_scheme::custom_dyn_scheme;
pub use dyn_scheme::{DynAsset, DynAssetRegistry};
