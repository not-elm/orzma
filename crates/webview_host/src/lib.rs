//! Tokio-free host integration for ozmux: a per-handle runtime root for the
//! webview control plane and (behind the `cef` feature) serving
//! dynamically-registered Tier 1 webview assets through an `ozma://`
//! custom scheme via `WebviewAssetRegistry`.

pub mod asset;
pub mod host;
pub mod ozma_scheme;

#[cfg(feature = "cef")]
pub use ozma_scheme::custom_ozma_scheme;
pub use ozma_scheme::{WebviewAsset, WebviewAssetRegistry};
