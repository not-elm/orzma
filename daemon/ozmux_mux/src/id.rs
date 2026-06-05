//! Typed, generational, session-scoped IDs for the multiplexer arena.
//!
//! Each is a `slotmap` key: removing a slot bumps its generation so a stale
//! key fails lookups (this replaces the Bevy dangling-reference observer).
//! `serde` here is for the IN-PROCESS core; the persistent/cross-restart
//! wire representation is `ozmux_proto`'s concern (Plan 3).

use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

new_key_type! {
    /// Attach/detach + persistence unit (tmux Session).
    pub struct SessionId;
    /// Full-screen tab within a session (tmux Window).
    pub struct WorkspaceId;
    /// Internal binary split node.
    pub struct SplitId;
    /// Leaf layout slot hosting surfaces.
    pub struct PaneId;
    /// A pane's content (terminal / webview).
    pub struct SurfaceId;
}

/// A layout-tree node address: either an internal split or a leaf pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeId {
    /// Internal split node.
    Split(SplitId),
    /// Leaf pane node.
    Pane(PaneId),
}
