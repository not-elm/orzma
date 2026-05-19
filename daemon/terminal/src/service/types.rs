//! Public value types crossing the `TerminalService` API surface.

use crate::vt::frame::Cursor;
use crate::vt::frame_ring::WireMessage;
use bytes::Bytes;
use ozmux_multiplexer::{SessionId, WindowId};
use tokio::sync::broadcast;

/// Configuration passed to [`crate::service::TerminalService::spawn`].
pub struct SpawnOptions {
    /// Terminal column count.
    pub cols: u16,
    /// Terminal row count.
    pub rows: u16,
    /// Shell program to launch (absolute path or `$PATH`-resolvable name).
    pub shell: String,
    /// Initial working directory for the spawned shell, if any.
    pub cwd: Option<String>,
    /// Owning Window id, surfaced to the spawned shell as `OZMUX_WINDOW_ID`.
    /// `None` only for callers that have no Window context (tests/legacy).
    pub window_id: Option<WindowId>,
    /// Owning Session id, surfaced to the spawned shell as `OZMUX_SESSION_ID`
    /// when present. Orphan Windows resolve to `None`.
    pub session_id: Option<SessionId>,
}

/// Current geometry and cursor state of an activity's terminal, returned by
/// [`crate::service::TerminalService::read_geometry`] for use in the hello frame.
pub struct TerminalGeometry {
    /// Terminal column count.
    pub cols: u16,
    /// Terminal row count.
    pub rows: u16,
    /// Cursor state at read time.
    pub cursor: Cursor,
    /// Wall-clock epoch micros captured when the VT bridge was constructed.
    /// `None` when `SystemTime` could not be captured (essentially never).
    pub bridge_started_at_unix_us: Option<u64>,
}

/// Outcome of subscribing to an activity's wire stream.
///
/// Callers render the snapshot or apply the replayed deltas, then consume
/// `rx` for all subsequent emissions without gaps.
pub enum FrameSubscription {
    /// Server emitted a fresh snapshot atomically with the subscription.
    /// Client should render the snapshot then consume `rx` for deltas.
    FreshSnapshot {
        /// Encoded MessagePack of the snapshot.
        snapshot: Bytes,
        /// Receiver for subsequent wire messages.
        rx: broadcast::Receiver<WireMessage>,
    },
    /// Server replayed buffered deltas covering `[last_seq+1, latest]`.
    /// Client applies each delta in order then consumes `rx` for further
    /// deltas.
    ResumeReplay {
        /// Buffered wire messages in insertion order, preserving WS opcode.
        deltas: Vec<WireMessage>,
        /// Maximum binary seq number among entries in `deltas`. Falls back to
        /// the caller's `last_seq` when the replay batch contains no binary
        /// frames (mode/error only).
        last_replay_seq: u32,
        /// Receiver for subsequent wire messages.
        rx: broadcast::Receiver<WireMessage>,
    },
}
