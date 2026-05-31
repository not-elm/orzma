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
pub mod scheme;

pub use bridge::{ControlExtension, ExtensionControlPlugin, ExtensionControlSet, terminal_env};
pub use command::{CommandExtension, CommandExtensionConfig};
pub use control::{
    ActivityKindSpec, ActivitySpec, ControlError, ControlOp, ControlOrientation, ControlParseError,
    ControlRequest, ControlResponse, ControlSide, SplitParams, SplitReply, encode_response,
    parse_call,
};
pub use handlers_bridge::{AidFrame, HandlersBridge};
pub use manifest::{Manifest, ManifestError};
pub use path_prefix::extension_path_prefix;
pub use protocol::{ProtocolError, Request, Response};
