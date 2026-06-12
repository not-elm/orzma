//! Tokio-free host integration for ozmux: discovers `ozmux.toml` extensions,
//! spawns the single Node host process, speaks NDJSON RPC over its Unix socket,
//! and (behind the `cef` feature) serves extension assets directly from disk
//! through a `bevy_cef` `ozmux-ext://` custom scheme.

pub mod asset;
pub mod error;
pub mod extension_discovery;
pub mod extension_manifest;
pub mod host;
pub mod host_descriptor;
pub mod host_process;
pub mod registry;
pub mod rpc_client;
pub mod scheme;

pub use error::{ExtensionError, ExtensionResult};
pub use extension_discovery::{DiscoveredExtension, discover_extensions};
pub use extension_manifest::{ExtensionManifest, ExtensionView};
pub use host_descriptor::{BuiltHostManifest, ExtensionDescriptorJson, HostManifestJson};
pub use host_process::{HostProcess, PreparedHost};
pub use registry::{RegisteredView, ViewId, ViewRegistry};
pub use rpc_client::HostRpcClient;
