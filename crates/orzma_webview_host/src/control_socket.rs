//! The channel the control socket's listener feeds the host through: the
//! events it sends, the id of each connection, and the socket's path.

use crate::boundary::HandleId;
use crate::error::RegisterError;
use crate::host::ValidatedRegistration;
use crate::protocol::ServerMsg;
#[cfg(any(test, feature = "test-support"))]
use crossbeam_channel::unbounded;
use crossbeam_channel::{Receiver, Sender};
use std::path::{Path, PathBuf};

/// The id the listener gives one accepted connection. Never reused within
/// one listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionId(u64);

impl ConnectionId {
    /// The connection id spelled `raw`.
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }
}

/// A request line, or a change in a connection's life, that the listener
/// hands the host.
#[derive(Debug)]
pub enum ControlEvent {
    /// A connection sent its `hello`. The host answers on `reply` whether
    /// `token` resolved to a live pane, and keeps `writer` for the lines it
    /// sends that connection.
    Hello {
        /// The connection that sent the line.
        connection: ConnectionId,
        /// The `$ORZMA_TOKEN` the program presented.
        token: String,
        /// The channel the connection's writer thread drains.
        writer: Sender<String>,
        /// Where the host answers.
        reply: Sender<bool>,
    },
    /// A `register`, validated on the listener thread. The host answers on
    /// `reply`.
    Register {
        /// The connection that sent the line.
        connection: ConnectionId,
        /// The validated content, or the reason it is invalid.
        registration: Result<ValidatedRegistration, RegisterError>,
        /// Where the host answers.
        reply: Sender<ServerMsg>,
    },
    /// A `new_instance`. The host answers on `reply`.
    NewInstance {
        /// The connection that sent the line.
        connection: ConnectionId,
        /// The handle to mint an instance for.
        handle: HandleId,
        /// Where the host answers.
        reply: Sender<ServerMsg>,
    },
    /// An `unregister`.
    Unregister {
        /// The connection that sent the line.
        connection: ConnectionId,
        /// The handle to release.
        handle: HandleId,
    },
    /// The connection closed.
    Disconnect {
        /// The connection that closed.
        connection: ConnectionId,
    },
    /// A socket `mount`.
    Mount {
        /// The connection that sent the line.
        connection: ConnectionId,
        /// The instance, in its wire spelling.
        instance: String,
        /// 0-based visible row of the rect's top edge.
        row: u16,
        /// 0-based column of the rect's left edge.
        col: u16,
        /// Rect height in cells.
        rows: u16,
        /// Rect width in cells.
        cols: u16,
    },
    /// A socket `unmount`.
    Unmount {
        /// The connection that sent the line.
        connection: ConnectionId,
        /// The instance, in its wire spelling.
        instance: String,
    },
}

/// The host's end of the control socket: the path programs connect to, and
/// the events its listener sends.
pub struct ControlSocket {
    sock_path: PathBuf,
    events: Receiver<ControlEvent>,
}

impl ControlSocket {
    /// A socket with no listener behind it: the returned sender stands in
    /// for the listener, and `sock_path` is only advertised to panes.
    #[cfg(any(test, feature = "test-support"))]
    pub fn injected(sock_path: impl Into<PathBuf>) -> (Self, Sender<ControlEvent>) {
        let (events_tx, events) = unbounded();
        let socket = Self {
            sock_path: sock_path.into(),
            events,
        };
        (socket, events_tx)
    }

    /// The path programs connect to (`$ORZMA_SOCK`).
    pub fn sock_path(&self) -> &Path {
        &self.sock_path
    }

    /// The events the listener sends.
    pub fn events(&self) -> &Receiver<ControlEvent> {
        &self.events
    }
}
