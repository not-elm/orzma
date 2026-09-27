//! The connections that completed their `hello`: each one's pane, and the
//! channel its writer thread drains.

use crate::control_socket::ConnectionId;
use crate::error::{Refusal, WebviewHostResult};
use crate::host::PaneKey;
use crate::protocol::PushMsg;
use crossbeam_channel::Sender;
use std::collections::HashMap;

/// Every hello'd connection's pane and writer channel.
pub(crate) struct Connections<P> {
    by_id: HashMap<ConnectionId, Connection<P>>,
}

impl<P> Default for Connections<P> {
    /// No connection.
    fn default() -> Self {
        Self {
            by_id: HashMap::new(),
        }
    }
}

impl<P: PaneKey> Connections<P> {
    /// Records that `connection` belongs to `pane` and writes through
    /// `writer`.
    pub fn insert(&mut self, connection: ConnectionId, pane: P, writer: Sender<String>) {
        self.by_id.insert(connection, Connection { pane, writer });
    }

    /// Forgets `connection`, dropping the host's end of its writer channel.
    pub fn remove(&mut self, connection: ConnectionId) {
        self.by_id.remove(&connection);
    }

    /// The pane `connection` belongs to, or `None` when it never completed
    /// its `hello` or has closed.
    pub fn pane_of(&self, connection: ConnectionId) -> Option<P> {
        self.by_id.get(&connection).map(|c| c.pane)
    }

    /// Queues `message` to `connection` as one NDJSON line.
    ///
    /// # Errors
    ///
    /// Returns [`Refusal::ConnectionClosed`] when the connection is unknown
    /// or its writer has exited, and a JSON error when `message` fails to
    /// serialize.
    pub fn push(&self, connection: ConnectionId, message: &PushMsg) -> WebviewHostResult {
        let line = serde_json::to_string(message)?;
        let target = self
            .by_id
            .get(&connection)
            .ok_or(Refusal::ConnectionClosed)?;
        target
            .writer
            .send(line)
            .map_err(|_| Refusal::ConnectionClosed)?;
        Ok(())
    }
}

/// One hello'd connection.
struct Connection<P> {
    pane: P,
    writer: Sender<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WebviewHostError;
    use crossbeam_channel::unbounded;

    fn focus_push() -> PushMsg {
        PushMsg::FocusChanged {
            handle: "h1".into(),
            instance: "i1".into(),
            focused: true,
        }
    }

    /// Asserts that a push reaches the connection's writer as one JSON line
    /// without a trailing newline.
    ///
    /// Case: the host tells a program that its page took the keyboard.
    #[test]
    fn a_push_reaches_the_writer_as_one_line() {
        let mut connections = Connections::default();
        let (writer, lines) = unbounded();
        connections.insert(ConnectionId::new(1), 1_u32, writer);
        connections
            .push(ConnectionId::new(1), &focus_push())
            .expect("the writer is alive");
        let line = lines.try_recv().expect("one line");
        assert!(!line.ends_with('\n'));
        assert!(line.contains(r#""op":"focus_changed""#));
    }

    /// Asserts that a push to an unknown connection, or to one whose writer
    /// has exited, is refused as closed.
    ///
    /// Case: a program exits between its page gaining focus and the host
    /// reporting it.
    #[test]
    fn a_push_to_a_closed_connection_is_refused() {
        let mut connections = Connections::default();
        assert!(matches!(
            connections.push(ConnectionId::new(9), &focus_push()),
            Err(WebviewHostError::Refused(Refusal::ConnectionClosed))
        ));
        let (writer, lines) = unbounded();
        connections.insert(ConnectionId::new(1), 1_u32, writer);
        drop(lines);
        assert!(matches!(
            connections.push(ConnectionId::new(1), &focus_push()),
            Err(WebviewHostError::Refused(Refusal::ConnectionClosed))
        ));
    }
}
