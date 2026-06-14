//! Sans-IO protocol core: turns tmux byte chunks into [`ClientEvent`]s and
//! encodes outgoing commands.

use crate::error::{TmuxError, TmuxResult};
use std::collections::VecDeque;
use tmux_control_parser::{BlockAssembler, ControlEvent, Frame};

// NOTE: `tmux -CC` wraps its entire control stream in a DCS sequence —
// `ESC P 1000 p` … `ESC \`. The introducer is glued to the first `%begin`, so
// without stripping it that first line fails to parse and the stream desyncs.
const DCS_INTRODUCER: &[u8] = b"\x1bP1000p";
const DCS_TERMINATOR: &[u8] = b"\x1b\\";

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

    /// Feeds a raw byte chunk; returns the events it produced (possibly empty).
    ///
    /// Splits on `\n` (stripping a trailing `\r`), buffers any incomplete tail,
    /// skips empty lines, and drives the assembler with each complete line.
    /// Strips the `tmux -CC` DCS introducer from the first line and ignores a
    /// bare DCS terminator line.
    pub fn feed(&mut self, bytes: &[u8]) -> TmuxResult<Vec<ClientEvent>> {
        self.line_buf.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(nl) = self.line_buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.line_buf.drain(..=nl).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let content = line.strip_prefix(DCS_INTRODUCER).unwrap_or(line.as_slice());
            if content.is_empty() || content == DCS_TERMINATOR {
                continue;
            }
            if let Some(event) = self.feed_line(content)? {
                events.push(event);
            }
        }
        Ok(events)
    }

    /// Pre-registers a pending command with no outgoing bytes.
    ///
    /// Used by the transport for tmux's launch subcommand (`new-session` /
    /// `attach-session`), whose reply block arrives before any client `send`.
    /// tmux assigns it an arbitrary command number, so correlation is by FIFO.
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
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    fn feed_line(&mut self, line: &[u8]) -> TmuxResult<Option<ClientEvent>> {
        match self.assembler.feed(line)? {
            Some(Frame::Reply { number, ok, body }) => {
                let id = self
                    .pending
                    .pop_front()
                    .ok_or(TmuxError::UnsolicitedReply { number })?;
                Ok(Some(ClientEvent::CommandComplete {
                    id,
                    number,
                    ok,
                    output: body,
                }))
            }
            Some(Frame::Notification(event)) => Ok(Some(ClientEvent::Notification(event))),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::{PaneId, WindowId};

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

    #[test]
    fn feed_single_notification_line() {
        let mut c = ProtocolClient::new();
        let events = c.feed(b"%window-add @1\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(1)
            })]
        );
    }

    #[test]
    fn feed_multiple_lines_one_chunk() {
        let mut c = ProtocolClient::new();
        let events = c.feed(b"%window-add @1\n%window-close @1\n").unwrap();
        assert_eq!(
            events,
            vec![
                ClientEvent::Notification(ControlEvent::WindowAdd {
                    window: WindowId(1)
                }),
                ClientEvent::Notification(ControlEvent::WindowClose {
                    window: WindowId(1)
                }),
            ]
        );
    }

    #[test]
    fn feed_line_split_across_chunks() {
        let mut c = ProtocolClient::new();
        assert!(c.feed(b"%window-add ").unwrap().is_empty());
        let events = c.feed(b"@7\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(7)
            })]
        );
    }

    #[test]
    fn feed_arbitrary_byte_boundaries_match_whole() {
        let whole = b"%begin 1 1 0\nbody-line\n%end 1 1 0\n";
        let mut a = ProtocolClient::new();
        a.register_pending();
        let whole_events = a.feed(whole).unwrap();

        let mut b = ProtocolClient::new();
        b.register_pending();
        let mut piece_events = Vec::new();
        for byte in whole.iter() {
            piece_events.extend(b.feed(&[*byte]).unwrap());
        }
        assert_eq!(whole_events, piece_events);
        assert_eq!(whole_events.len(), 1);
    }

    #[test]
    fn feed_partial_line_buffered_no_event() {
        let mut c = ProtocolClient::new();
        assert!(c.feed(b"%window-add @1").unwrap().is_empty());
    }

    #[test]
    fn feed_empty_chunk_no_change() {
        let mut c = ProtocolClient::new();
        assert!(c.feed(b"").unwrap().is_empty());
    }

    #[test]
    fn feed_strips_trailing_cr() {
        let mut c = ProtocolClient::new();
        let events = c.feed(b"%window-add @2\r\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(2)
            })]
        );
    }

    #[test]
    fn feed_skips_blank_lines() {
        let mut c = ProtocolClient::new();
        let events = c.feed(b"\n\r\n%window-add @3\n\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(3)
            })]
        );
    }

    #[test]
    fn feed_strips_dcs_wrapper() {
        // Mirrors a real `tmux -CC` startup: the DCS introducer is glued to the
        // first %begin, and the terminator arrives as a bare line. CRLF endings.
        let mut c = ProtocolClient::new();
        let id = c.register_pending();
        let events = c
            .feed(b"\x1bP1000p%begin 1 318 0\r\n%end 1 318 0\r\n%window-add @0\r\n\x1b\\\r\n")
            .unwrap();
        assert_eq!(
            events,
            vec![
                ClientEvent::CommandComplete { id, number: 318, ok: true, output: vec![] },
                ClientEvent::Notification(ControlEvent::WindowAdd { window: WindowId(0) }),
            ]
        );
    }

    #[test]
    fn reply_correlates_to_pending_command() {
        let mut c = ProtocolClient::new();
        let id = c.send("list-panes").unwrap();
        let _ = c.take_outgoing();
        let events = c.feed(b"%begin 100 5 0\n0: ksh\n%end 100 5 0\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::CommandComplete {
                id,
                number: 5,
                ok: true,
                output: vec!["0: ksh".to_string()],
            }]
        );
        assert_eq!(c.pending_len(), 0);
    }

    #[test]
    fn error_reply_is_not_ok() {
        let mut c = ProtocolClient::new();
        let id = c.send("bogus").unwrap();
        let _ = c.take_outgoing();
        let events = c
            .feed(b"%begin 1 9 0\nunknown command\n%error 1 9 0\n")
            .unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::CommandComplete {
                id,
                number: 9,
                ok: false,
                output: vec!["unknown command".to_string()],
            }]
        );
    }

    #[test]
    fn empty_reply_body() {
        let mut c = ProtocolClient::new();
        let id = c.send("new-window").unwrap();
        let _ = c.take_outgoing();
        let events = c.feed(b"%begin 1 2 0\n%end 1 2 0\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::CommandComplete {
                id,
                number: 2,
                ok: true,
                output: vec![]
            }]
        );
    }

    #[test]
    fn two_commands_correlate_fifo() {
        let mut c = ProtocolClient::new();
        let a = c.send("a").unwrap();
        let b = c.send("b").unwrap();
        let _ = c.take_outgoing();
        let events = c
            .feed(b"%begin 1 1 0\nA\n%end 1 1 0\n%begin 1 2 0\nB\n%end 1 2 0\n")
            .unwrap();
        assert_eq!(
            events,
            vec![
                ClientEvent::CommandComplete {
                    id: a,
                    number: 1,
                    ok: true,
                    output: vec!["A".into()]
                },
                ClientEvent::CommandComplete {
                    id: b,
                    number: 2,
                    ok: true,
                    output: vec!["B".into()]
                },
            ]
        );
    }

    #[test]
    fn notification_between_reply_blocks() {
        let mut c = ProtocolClient::new();
        let a = c.send("a").unwrap();
        let b = c.send("b").unwrap();
        let _ = c.take_outgoing();
        let events = c
            .feed(b"%begin 1 1 0\nA\n%end 1 1 0\n%window-add @9\n%begin 1 2 0\nB\n%end 1 2 0\n")
            .unwrap();
        assert_eq!(
            events,
            vec![
                ClientEvent::CommandComplete {
                    id: a,
                    number: 1,
                    ok: true,
                    output: vec!["A".into()]
                },
                ClientEvent::Notification(ControlEvent::WindowAdd {
                    window: WindowId(9)
                }),
                ClientEvent::CommandComplete {
                    id: b,
                    number: 2,
                    ok: true,
                    output: vec!["B".into()]
                },
            ]
        );
    }

    #[test]
    fn unsolicited_reply_errors() {
        let mut c = ProtocolClient::new();
        let err = c.feed(b"%begin 1 4 0\n%end 1 4 0\n").unwrap_err();
        assert!(matches!(err, TmuxError::UnsolicitedReply { number: 4 }));
    }

    #[test]
    fn command_complete_carries_tmux_number() {
        let mut c = ProtocolClient::new();
        let id = c.send("x").unwrap();
        let _ = c.take_outgoing();
        let events = c.feed(b"%begin 1 42 0\n%end 1 42 0\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::CommandComplete {
                id,
                number: 42,
                ok: true,
                output: vec![]
            }]
        );
    }

    #[test]
    fn startup_block_via_register_pending() {
        let mut c = ProtocolClient::new();
        let id = c.register_pending();
        let events = c.feed(b"%begin 1 1 1\n%end 1 1 1\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::CommandComplete {
                id,
                number: 1,
                ok: true,
                output: vec![]
            }]
        );
    }

    #[test]
    fn output_notification_passthrough() {
        let mut c = ProtocolClient::new();
        let events = c.feed(b"%output %0 hi\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::Output {
                pane: PaneId(0),
                data: b"hi".to_vec(),
            })]
        );
    }

    #[test]
    fn exit_notification_passthrough() {
        let mut c = ProtocolClient::new();
        let events = c.feed(b"%exit\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::Exit {
                reason: None
            })]
        );
    }

    #[test]
    fn layout_change_passthrough() {
        // NOTE: lenient checksum; if this string ever fails to parse, copy a
        // known-good `%layout-change` line from the parser's layout tests in
        // crates/tmux_control_parser/src/event.rs.
        let mut c = ProtocolClient::new();
        let events = c
            .feed(b"%layout-change @0 abcd,80x24,0,0,0 abcd,80x24,0,0,0 *\n")
            .unwrap();
        assert!(matches!(
            events.as_slice(),
            [ClientEvent::Notification(ControlEvent::LayoutChange { .. })]
        ));
    }

    #[test]
    fn unknown_notification_passthrough() {
        let mut c = ProtocolClient::new();
        let events = c.feed(b"%future-thing some args\n").unwrap();
        assert!(matches!(
            events.as_slice(),
            [ClientEvent::Notification(ControlEvent::Unknown { .. })]
        ));
    }

    #[test]
    fn malformed_line_outside_block_errors() {
        let mut c = ProtocolClient::new();
        let err = c.feed(b"hello world\n").unwrap_err();
        assert!(matches!(err, TmuxError::Parse(_)));
    }

    #[test]
    fn malformed_body_line_collected_verbatim() {
        let mut c = ProtocolClient::new();
        let id = c.send("x").unwrap();
        let _ = c.take_outgoing();
        let events = c.feed(b"%begin 1 1 0\nhello world\n%end 1 1 0\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::CommandComplete {
                id,
                number: 1,
                ok: true,
                output: vec!["hello world".to_string()],
            }]
        );
    }

    #[test]
    fn glued_end_does_not_close_block_known_limitation() {
        // NOTE: tmux #2215: a body line without a trailing newline can glue `%end` to
        // the last body line. The current BlockAssembler treats it as body and
        // the block stays open. This guards that documented behavior.
        let mut c = ProtocolClient::new();
        c.send("capture").unwrap();
        let _ = c.take_outgoing();
        let events = c.feed(b"%begin 1 1 0\nlast-line%end 1 1 0\n").unwrap();
        assert!(events.is_empty());
        assert_eq!(c.pending_len(), 1);
    }
}
