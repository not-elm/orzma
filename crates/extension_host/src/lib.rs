//! Tokio-free host for ozmux Node extensions: spawns an extension process,
//! speaks a minimal length-prefixed byte protocol over its Unix socket, and
//! (behind the `cef` feature) bridges its UI bytes through a `bevy_cef`
//! `ozmux-ext://` custom scheme.

pub mod bridge;
pub mod command;
pub mod control;
pub mod handlers_bridge;
pub mod host;
pub mod host_descriptor;
pub mod host_process;
pub mod manifest;
pub mod path_prefix;
pub mod plugin_discovery;
pub mod plugin_manifest;
pub mod protocol;
pub mod registry;
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
pub use handlers_bridge::{HandlersBridge, SurfaceIdFrame};
pub use host_descriptor::{
    BuiltHostManifest, HostManifestJson, PluginDescriptorJson, build_host_manifest,
};
pub use host_process::{HostProcess, PreparedHost, prepare_host_runtime};
pub use manifest::{Manifest, ManifestError};
pub use path_prefix::extension_path_prefix;
pub use plugin_discovery::{DiscoveredPlugin, discover_plugins};
pub use plugin_manifest::{ManifestView, PluginManifest, PluginManifestError};
pub use protocol::{ProtocolError, Request, Response};
pub use registry::{RegisteredView, ViewRegistry};
