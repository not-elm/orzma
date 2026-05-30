//! Tokio-free host for ozmux Node extensions: spawns an extension process,
//! speaks a minimal length-prefixed byte protocol over its Unix socket, and
//! (behind the `cef` feature) bridges its UI bytes through a `bevy_cef`
//! `ozmux-ext://` custom scheme.

pub mod host;
pub mod path_prefix;
pub mod protocol;
pub mod scheme;

pub use host::{
    ExtensionConfig, ExtensionEndpoints, ExtensionHost, FetchError, HostError, HostResult,
    LifecycleEvent, fetch,
};
pub use path_prefix::extension_path_prefix;
pub use protocol::{ProtocolError, Request, Response};
