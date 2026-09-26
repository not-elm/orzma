//! The channel the control socket's listener feeds the host through: the
//! events it sends, the id of each connection, and the socket's path.

use crate::boundary::{ForwardChord, HandleId};
use crate::error::{RegisterError, WebviewHostResult};
use crate::host::ValidatedRegistration;
use crate::listener::spawn_listener;
use crate::protocol::{NavAction, ServerMsg};
use crate::runtime_root::RuntimeRoot;
#[cfg(any(test, feature = "test-support"))]
use crossbeam_channel::unbounded;
use crossbeam_channel::{Receiver, Sender};
use serde_json::Value;
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
    /// A socket `focus`.
    Focus {
        /// The connection that sent the line.
        connection: ConnectionId,
        /// The instance to focus in its wire spelling, or `None` to blur.
        instance: Option<String>,
    },
    /// A program's reply to a host-initiated `call`.
    Reply {
        /// The connection that sent the line.
        connection: ConnectionId,
        /// The global reqId the host assigned to the call.
        req_id: String,
        /// Whether the call succeeded.
        ok: bool,
        /// The success value.
        value: Value,
        /// The error message when `ok` is false.
        error: Option<String>,
    },
    /// A program-initiated event for the mounted pages of `handle`.
    Emit {
        /// The connection that sent the line.
        connection: ConnectionId,
        /// The handle whose pages receive the event.
        handle: HandleId,
        /// The event name.
        event: String,
        /// The event payload.
        payload: Value,
    },
    /// A socket `navigate`.
    Navigate {
        /// The connection that sent the line.
        connection: ConnectionId,
        /// The instance, in its wire spelling.
        instance: String,
        /// What to do.
        action: NavAction,
    },
    /// A `set_forward_keys`.
    SetForwardKeys {
        /// The connection that sent the line.
        connection: ConnectionId,
        /// The handle whose chords are replaced.
        handle: HandleId,
        /// The complete new chord list.
        keys: Vec<ForwardChord>,
    },
}

/// The host's end of the control socket: the path programs connect to, and
/// the events its listener sends.
pub struct ControlSocket {
    sock_path: PathBuf,
    events: Receiver<ControlEvent>,
    #[expect(
        dead_code,
        reason = "held only for its Drop impl, which removes the socket directory"
    )]
    runtime: Option<RuntimeRoot>,
}

impl ControlSocket {
    /// Resolves the runtime directory `<parent>/<pid>/control/`, binds its
    /// `control.sock`, and starts the listener. Dropping the socket removes
    /// the directory; the listener threads stay parked until the process
    /// exits.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeRootError`](crate::error::RuntimeRootError) when no
    /// candidate directory keeps the socket path within `sun_path`, and an
    /// I/O error when a directory cannot be created or restricted or the
    /// socket cannot be bound.
    pub fn bind(parent: &Path, pid: u32) -> WebviewHostResult<Self> {
        let runtime = RuntimeRoot::resolve_in(parent, pid, "control")?;
        let sock_path = runtime.socket_path("control");
        let events = spawn_listener(&sock_path)?;
        Ok(Self {
            sock_path,
            events,
            runtime: Some(runtime),
        })
    }

    /// A socket with no listener behind it: the returned sender stands in
    /// for the listener, and `sock_path` is only advertised to panes.
    #[cfg(any(test, feature = "test-support"))]
    pub fn injected(sock_path: impl Into<PathBuf>) -> (Self, Sender<ControlEvent>) {
        let (events_tx, events) = unbounded();
        let socket = Self {
            sock_path: sock_path.into(),
            events,
            runtime: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uds::UnixStream;
    use std::io::Write;
    use std::time::Duration;

    /// Asserts that a bound socket lives at `<runtime root>/sock/control.sock`
    /// and hands a connection's `hello` to the host.
    ///
    /// Case: orzma starts and a pane's shell connects.
    #[test]
    fn a_bound_socket_hands_a_hello_to_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let socket = ControlSocket::bind(dir.path(), 4245).expect("the socket binds");
        assert!(socket.sock_path().ends_with("control/sock/control.sock"));
        let mut client = UnixStream::connect(socket.sock_path()).unwrap();
        writeln!(client, r#"{{"op":"hello","token":"tok"}}"#).unwrap();
        client.flush().unwrap();
        let event = socket
            .events()
            .recv_timeout(Duration::from_secs(2))
            .expect("the hello arrives");
        assert!(matches!(event, ControlEvent::Hello { .. }));
    }
}
