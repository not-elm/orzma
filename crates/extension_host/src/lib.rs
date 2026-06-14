//! Tokio-free host integration for ozmux: spawns the single Node host process,
//! speaks NDJSON RPC over its Unix socket (host RPC plumbing, currently dormant),
//! and (behind the `cef` feature) serves dynamically-registered Tier 1 webview
//! assets through an `ozmux-dyn://` custom scheme via `DynAssetRegistry`.

pub mod asset;
pub mod dyn_scheme;
pub mod host;
pub mod host_process;
pub mod rpc_client;

#[cfg(feature = "cef")]
pub use dyn_scheme::custom_dyn_scheme;
pub use dyn_scheme::{DynAsset, DynAssetRegistry};
pub use host_process::{HostProcess, PreparedHost};
pub use rpc_client::HostRpcClient;
