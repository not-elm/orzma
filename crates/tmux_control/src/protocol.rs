//! Sans-IO protocol core: turns tmux byte chunks into [`ClientEvent`]s and
//! encodes outgoing commands.

use std::collections::VecDeque;
use tmux_control_parser::{BlockAssembler, ControlEvent, Frame};
use crate::error::{TmuxError, TmuxResult};

/// Library-assigned handle for an in-flight command (not tmux's command number).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub u64);

/// A higher-level event surfaced to the consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    /// A command's reply block completed.
    CommandComplete {
        /// The library-assigned id from [`ProtocolClient::send`].
        id: CommandId,
        /// tmux's own command number (for auditing; correlation is pure FIFO).
        number: u32,
        /// `true` if closed by `%end`, `false` if by `%error`.
        ok: bool,
        /// Reply body lines, verbatim, in order.
        output: Vec<String>,
    },
    /// A standalone notification, passed through from the parser.
    Notification(ControlEvent),
}

/// Sans-IO core driving a [`BlockAssembler`] with command/reply correlation.
#[derive(Debug, Default)]
pub struct ProtocolClient {
    assembler: BlockAssembler,
    line_buf: Vec<u8>,
    pending: VecDeque<CommandId>,
    next_id: u64,
    outgoing: Vec<u8>,
}

impl ProtocolClient {
    /// Returns a fresh client with no pending commands.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a pending command and queues `command\n` into the outgoing buffer.
    ///
    /// Rejects a `command` containing `\n`/`\r` with [`TmuxError::InvalidCommand`]
    /// to preserve the one-command-to-one-reply invariant.
    pub fn send(&mut self, command: &str) -> TmuxResult<CommandId> {
        if command.contains('\n') || command.contains('\r') {
            return Err(TmuxError::InvalidCommand);
        }
        let id = self.register_pending();
        self.outgoing.extend_from_slice(command.as_bytes());
        self.outgoing.push(b'\n');
        Ok(id)
    }

    /// Drains the outgoing buffer for the transport to write to tmux's stdin.
    pub fn take_outgoing(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.outgoing)
    }

    /// Pre-registers a pending command with no outgoing bytes.
    ///
    /// Used by the transport for tmux's launch subcommand (`new-session` /
    /// `attach-session`), whose reply block (`number` 1) arrives before any
    /// client `send`.
    pub(crate) fn register_pending(&mut self) -> CommandId {
        let id = CommandId(self.next_id);
        self.next_id += 1;
        self.pending.push_back(id);
        id
    }

    /// Removes `id` from the back of the pending queue if it is still last.
    ///
    /// Used to undo a [`send`](Self::send) registration when the subsequent
    /// transport write fails.
    pub(crate) fn rollback_last_pending(&mut self, id: CommandId) {
        if self.pending.back() == Some(&id) {
            self.pending.pop_back();
        }
    }

    /// Number of commands awaiting a reply (test/diagnostic accessor).
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_assigns_id_and_queues_line() {
        let mut c = ProtocolClient::new();
        let id = c.send("list-panes").unwrap();
        assert_eq!(id, CommandId(0));
        assert_eq!(c.take_outgoing(), b"list-panes\n".to_vec());
    }

    #[test]
    fn take_outgoing_drains() {
        let mut c = ProtocolClient::new();
        c.send("a").unwrap();
        assert_eq!(c.take_outgoing(), b"a\n".to_vec());
        assert!(c.take_outgoing().is_empty());
    }

    #[test]
    fn multiple_sends_preserve_order() {
        let mut c = ProtocolClient::new();
        let a = c.send("first").unwrap();
        let b = c.send("second").unwrap();
        assert_eq!((a, b), (CommandId(0), CommandId(1)));
        assert_eq!(c.take_outgoing(), b"first\nsecond\n".to_vec());
        assert_eq!(c.pending_len(), 2);
    }

    #[test]
    fn command_ids_monotonic() {
        let mut c = ProtocolClient::new();
        let ids: Vec<_> = (0..3).map(|_| c.send("x").unwrap()).collect();
        assert_eq!(ids, vec![CommandId(0), CommandId(1), CommandId(2)]);
    }

    #[test]
    fn send_rejects_embedded_newline() {
        let mut c = ProtocolClient::new();
        assert!(matches!(c.send("a\nb"), Err(TmuxError::InvalidCommand)));
        assert!(matches!(c.send("a\rb"), Err(TmuxError::InvalidCommand)));
        assert_eq!(c.pending_len(), 0);
        assert!(c.take_outgoing().is_empty());
    }

    #[test]
    fn rollback_removes_last_pending() {
        let mut c = ProtocolClient::new();
        let id = c.send("x").unwrap();
        let _ = c.take_outgoing();
        c.rollback_last_pending(id);
        assert_eq!(c.pending_len(), 0);
    }
}
