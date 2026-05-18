//! Shared identifier and value types used in both wire schemas
//! (HostCommand/HostEvent and BrowserClient/ServerMsg).

use serde::{Deserialize, Serialize};

/// Activity identifier (mirrors `ozmux_multiplexer::ActivityId` as a String wrapper).
/// Defined here separately to avoid pulling the multiplexer crate into cef_host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActivityId(pub String);

/// Rectangle in pixel coordinates (upper-left origin).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Composite frame identity. Used for reconnect handshake and dedupe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameKey {
    pub session_id: u64,
    pub epoch: u32,
    pub frame_seq: u64,
}
