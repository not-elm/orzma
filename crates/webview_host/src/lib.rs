//! Tokio-free host integration for orzma: a per-handle runtime root for the
//! webview control plane and (behind the `cef` feature) serving
//! dynamically-registered Tier 1 webview assets through an `orzma://`
//! custom scheme via `WebviewAssetRegistry`.

pub mod asset;
pub mod host;
pub mod orzma_scheme;
pub mod private_dir;
pub mod uds;

#[cfg(feature = "cef")]
pub use orzma_scheme::custom_orzma_scheme;
pub use orzma_scheme::{WebviewAsset, WebviewAssetRegistry};
pub use private_dir::restrict_to_current_user;
