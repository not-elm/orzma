//! Tokio-free host for ozmux Node extensions: spawns an extension process,
//! speaks a minimal length-prefixed byte protocol over its Unix socket, and
//! (behind the `cef` feature) bridges its UI bytes through a `bevy_cef`
//! `ozmux-ext://` custom scheme.

pub mod protocol;

pub use protocol::{ProtocolError, Request, Response};
