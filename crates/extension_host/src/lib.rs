//! Tokio-free host for ozmux Node extensions: spawns an extension process,
//! speaks a minimal length-prefixed byte protocol over its Unix socket, and
//! (behind the `cef` feature) bridges its UI bytes through a `bevy_cef`
//! `ozmux-ext://` custom scheme.

pub mod bridge;
pub mod command;
pub mod control;
pub mod handlers_bridge;
pub mod host;
pub mod manifest;
pub mod path_prefix;
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
pub use registry::{RegisteredView, ViewRegistry};
pub use handlers_bridge::{HandlersBridge, SurfaceIdFrame};
pub use manifest::{Manifest, ManifestError};
pub use path_prefix::extension_path_prefix;
pub use protocol::{ProtocolError, Request, Response};
