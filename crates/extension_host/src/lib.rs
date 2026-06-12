//! Tokio-free host for ozmux Node extensions: spawns an extension process,
//! speaks a minimal length-prefixed byte protocol over its Unix socket, and
//! (behind the `cef` feature) bridges its UI bytes through a `bevy_cef`
//! `ozmux-ext://` custom scheme.

pub mod asset;
pub mod bridge;
pub mod command;
pub mod control;
pub mod error;
pub mod extension_discovery;
pub mod extension_manifest;
pub mod host;
pub mod host_descriptor;
pub mod host_process;
pub mod manifest;
pub mod path_prefix;
pub mod protocol;
pub mod registry;
pub mod rpc_client;
pub mod scheme;

pub use bridge::{
    ControlExtension, ExtensionControlPlugin, ExtensionControlSet, apply_control_request,
    terminal_env,
};
pub use command::{CommandExtension, CommandExtensionConfig};
pub use control::{
    ActivateParams, AddSurfaceParams, ControlError, ControlOp, ControlOrientation,
    ControlParseError, ControlReply, ControlRequest, ControlResponse, ControlSide,
    RegisterViewParams, SplitParams, SurfaceKindSpec, SurfaceSpec, encode_response, parse_call,
};
pub use error::{ExtensionError, ExtensionResult};
pub use extension_discovery::{DiscoveredExtension, discover_extensions};
pub use extension_manifest::{ExtensionManifest, ExtensionView};
pub use host_descriptor::{BuiltHostManifest, ExtensionDescriptorJson, HostManifestJson};
pub use host_process::{HostProcess, PreparedHost};
pub use manifest::{Manifest, ManifestError};
pub use path_prefix::extension_path_prefix;
pub use protocol::{ProtocolError, Request, Response};
pub use registry::{RegisteredView, ViewId, ViewRegistry};
pub use rpc_client::HostRpcClient;
