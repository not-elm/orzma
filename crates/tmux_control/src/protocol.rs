//! Sans-IO protocol core: turns tmux byte chunks into [`ClientEvent`]s and
//! encodes outgoing commands.

use crate::error::{TmuxError, TmuxResult};
use std::collections::VecDeque;
use tmux_control_parser::{BlockAssembler, ControlEvent, Frame};
use tracing::{debug, warn};

// NOTE: `tmux -CC` wraps its entire control stream in a DCS sequence —
// `ESC P 1000 p` … `ESC \`. The introducer is glued to the first `%begin`, so
// without stripping it that first line fails to parse and the stream desyncs.
const DCS_INTRODUCER: &[u8] = b"\x1bP1000p";
const DCS_TERMINATOR: &[u8] = b"\x1b\\";

/// Library-assigned handle for an in-flight command (not tmux's command number).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub u64);

/// A Layer-2 event: a Layer-1 protocol event, or transport termination.
#[derive(Debug)]
pub enum TransportEvent {
    /// A protocol event from Layer 1 (kept I/O-agnostic).
    Protocol(ClientEvent),
    /// The transport closed (process exit / EOF / reader I/O error).
    Closed {
        /// Human-readable reason (EOF marker or the I/O error text).
        reason: String,
    },
}

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

/// One slot in the command↔reply FIFO: a correlated query reply, or a drain that
/// swallows an effect command's reply blocks up to its fence line.
///
/// `fence` is the `OZMUXFENCE_<n>` line tmux echoes back, formatted once in
/// `send_effect`. The counter is internal `ProtocolClient` state, so no real
/// binding's output collides with it — it is not a secret, though: a binding that
/// deliberately emits the exact current line could end its own drain early
/// (self-sabotage, not a realistic accidental case).
#[derive(Debug)]
enum PendingSlot {
    /// A normal correlated query: the next `flags=1` block surfaces as its reply.
    Reply(CommandId),
    /// A fenced effect: consume `flags=1` blocks until the `ok` block whose body is
    /// exactly `fence`. `seen` counts drained blocks for the runaway backstop.
    Drain { fence: String, seen: u16 },
}

/// Safety cap: a single `Drain` consuming more than this many blocks without its
/// fence means the FIFO is desynced. The drain is then abandoned (popped) and a
/// warning logged — never a real binding, since the separate fence line always
/// terminates the drain.
const MAX_DRAIN_BLOCKS: u16 = 256;

/// Outcome of feeding one `flags=1` block to a front `Drain` slot.
enum DrainStep {
    /// Non-terminating block consumed; keep draining.
    Consumed,
    /// Fence token seen; the drain is complete.
    Done,
    /// Cap exceeded without the fence — abandon the runaway drain (`fence` token
    /// for the log, count drained).
    Runaway(String, u16),
}

/// Sans-IO core driving a [`BlockAssembler`] with command/reply correlation.
#[derive(Debug, Default)]
pub struct ProtocolClient {
    assembler: BlockAssembler,
    line_buf: Vec<u8>,
    pending: VecDeque<PendingSlot>,
    next_id: u64,
    next_fence: u64,
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

    /// Queues a fire-and-forget effect command (a relayed tmux binding) followed by
    /// a fence line, and registers a `Drain` slot so the command's reply blocks —
    /// however many tmux emits for a multi-statement binding — are consumed up to the
    /// fence instead of stealing later queries' reply slots.
    ///
    /// Rejects an embedded `\n`/`\r` like [`ProtocolClient::send`]. The fence is a
    /// SEPARATE line (not appended with `;`) so a parse error in `raw` cannot suppress
    /// it: the fence always parses, runs, and terminates the drain.
    pub fn send_effect(&mut self, raw: &str) -> TmuxResult<()> {
        if raw.contains('\n') || raw.contains('\r') {
            return Err(TmuxError::InvalidCommand);
        }
        let fence = format!("OZMUXFENCE_{}", self.next_fence);
        self.next_fence += 1;
        self.outgoing.extend_from_slice(raw.as_bytes());
        self.outgoing.push(b'\n');
        self.outgoing
            .extend_from_slice(format!("display-message -p {fence}").as_bytes());
        self.outgoing.push(b'\n');
        self.pending
            .push_back(PendingSlot::Drain { fence, seen: 0 });
        Ok(())
    }

    /// Feeds a raw byte chunk; returns the events it produced (possibly empty).
    ///
    /// Splits on `\n` (stripping a trailing `\r`), buffers any incomplete tail,
    /// treats a blank/whitespace-only line outside a block as a no-op (a blank
    /// line inside a block is kept as body), and drives the assembler with each
    /// complete line. Strips the `tmux -CC` DCS introducer from the first line
    /// and ignores a bare DCS terminator line.
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
            if content == DCS_TERMINATOR {
                continue;
            }
            if let Some(event) = self.feed_line(content)? {
                events.push(event);
            }
        }
        Ok(events)
    }

    /// Assigns the next [`CommandId`] and records it as awaiting a reply.
    fn register_pending(&mut self) -> CommandId {
        let id = CommandId(self.next_id);
        self.next_id += 1;
        self.pending.push_back(PendingSlot::Reply(id));
        id
    }

    /// Advances the front `Drain` slot by one `flags=1` reply block and returns the
    /// action the caller should take. Scopes the `front_mut` borrow to this call so
    /// the caller is free to `pop_front` afterward.
    fn step_drain(&mut self, ok: bool, body: &[String]) -> DrainStep {
        let Some(PendingSlot::Drain { fence, seen }) = self.pending.front_mut() else {
            unreachable!("front was Drain")
        };
        if ok && body.len() == 1 && body[0] == *fence {
            DrainStep::Done
        } else {
            *seen = seen.saturating_add(1);
            if *seen > MAX_DRAIN_BLOCKS {
                DrainStep::Runaway(fence.clone(), *seen)
            } else {
                DrainStep::Consumed
            }
        }
    }

    /// Number of commands awaiting a reply (test/diagnostic accessor).
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    fn feed_line(&mut self, line: &[u8]) -> TmuxResult<Option<ClientEvent>> {
        let frame = match self.assembler.feed(line) {
            Ok(frame) => frame,
            // A blank/whitespace-only line only errors outside a block; inside a
            // block the assembler keeps it as body. Treat it as a no-op here.
            Err(tmux_control_parser::TmuxError::Empty) => return Ok(None),
            // NOTE: a stray %end/%error outside a block means the stream is
            // desynchronised — propagate as fatal so the transport closes cleanly.
            Err(e @ tmux_control_parser::TmuxError::UnexpectedEnd { .. }) => {
                return Err(e.into());
            }
            // NOTE: any other error is a parse failure on a standalone notification
            // line (e.g. a %layout-change format from an older tmux version that
            // the parser does not recognise). Closing the transport over a single
            // bad notification line would blank all panes; warn and skip instead.
            Err(e) => {
                warn!(
                    line = ?String::from_utf8_lossy(line),
                    error = %e,
                    "skipping malformed tmux notification; transport remains open"
                );
                return Ok(None);
            }
        };
        match frame {
            Some(Frame::Reply {
                number,
                flags,
                ok,
                body,
            }) => {
                // NOTE: only a control-client command reply (flags bit 0 set) may
                // consume a pending slot. tmux also emits unsolicited blocks this
                // client never sent — the adopted-stream launch reply, and a
                // hook's `run-shell` output (e.g. `after-select-pane` in a
                // multi-pane session). Those carry flags=0; popping `pending` for
                // one would assign the next sent command's id to it and desync
                // every later reply (which silently freezes the copy-mode refresh
                // loop). Skipping without popping keeps FIFO correlation aligned.
                if !is_client_command_reply(flags) {
                    return Ok(None);
                }
                match self.pending.front() {
                    Some(PendingSlot::Reply(_)) => {
                        let Some(PendingSlot::Reply(id)) = self.pending.pop_front() else {
                            unreachable!("front was Reply")
                        };
                        Ok(Some(ClientEvent::CommandComplete {
                            id,
                            number,
                            ok,
                            output: body,
                        }))
                    }
                    Some(PendingSlot::Drain { .. }) => match self.step_drain(ok, &body) {
                        DrainStep::Consumed => Ok(None),
                        DrainStep::Done => {
                            self.pending.pop_front();
                            Ok(None)
                        }
                        DrainStep::Runaway(fence, drained) => {
                            // NOTE: recover locally (pop + log), never return an error — the
                            // sole production feed() caller swallows feed() errors, so an error
                            // would drop this chunk's already-collected events (incl. %output)
                            // and leave the slot wedged.
                            self.pending.pop_front();
                            warn!(
                                fence = %fence,
                                drained,
                                "fenced effect drain exceeded cap; abandoning fence (possible reply desync)"
                            );
                            Ok(None)
                        }
                    },
                    // NOTE: a flags=1 reply with no pending command means the FIFO
                    // is already desynced (more client replies than sent commands).
                    // Closing the transport over it would blank the whole
                    // projection; log at debug and skip so the connection stays
                    // alive.
                    None => {
                        debug!(
                            number,
                            "skipping client-command reply with no pending command; transport remains open"
                        );
                        Ok(None)
                    }
                }
            }
            Some(Frame::Notification(event)) => Ok(Some(ClientEvent::Notification(event))),
            None => Ok(None),
        }
    }
}

/// Returns whether a `%begin` flags bitmask marks a reply to a command the
/// control client sent.
///
/// tmux sets bit 0 only for commands read from the control client's own input
/// (its `CMDQ_STATE_CONTROL` flag, set on the control-client read path), and
/// clears it for the adopted-stream launch reply and for unsolicited internal
/// output (e.g. a hook's `run-shell`, as `after-select-pane` triggers). That bit
/// is exactly the FIFO precondition: only a client-issued command's reply may
/// consume a pending slot. The man page labels the field "currently not used",
/// but it is populated — the man page's own example shows `%begin … 1` for a
/// command reply; confirmed in `-CC` mode on tmux 3.6b.
fn is_client_command_reply(flags: u32) -> bool {
    flags & 1 != 0
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
        let whole = b"%begin 1 1 1\nbody-line\n%end 1 1 1\n";
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
    fn feed_skips_whitespace_only_lines_outside_block() {
        let mut c = ProtocolClient::new();
        let events = c.feed(b"   \n\t\n%window-add @4\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(4)
            })]
        );
    }

    #[test]
    fn feed_preserves_blank_body_lines_inside_block() {
        let mut c = ProtocolClient::new();
        let id = c.send("capture").unwrap();
        let _ = c.take_outgoing();
        let events = c
            .feed(b"%begin 1 1 1\nline1\n\nline3\n%end 1 1 1\n")
            .unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::CommandComplete {
                id,
                number: 1,
                ok: true,
                output: vec!["line1".to_string(), String::new(), "line3".to_string()],
            }]
        );
    }

    #[test]
    fn feed_strips_dcs_wrapper() {
        // Mirrors a real `tmux -CC` startup: the DCS introducer is glued to the
        // first %begin (the launch reply, flags=0), the terminator arrives as a
        // bare line, CRLF endings. The launch block is unsolicited (flags=0) and
        // skipped; only the notification passes through.
        let mut c = ProtocolClient::new();
        let events = c
            .feed(b"\x1bP1000p%begin 1 318 0\r\n%end 1 318 0\r\n%window-add @0\r\n\x1b\\\r\n")
            .unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(0)
            })]
        );
    }

    #[test]
    fn reply_correlates_to_pending_command() {
        let mut c = ProtocolClient::new();
        let id = c.send("list-panes").unwrap();
        let _ = c.take_outgoing();
        let events = c.feed(b"%begin 100 5 1\n0: ksh\n%end 100 5 1\n").unwrap();
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
    fn unsolicited_hook_block_does_not_steal_pending_slot() {
        // A tmux hook's `run-shell` (e.g. `after-select-pane`) emits an
        // unsolicited %begin/%end block with flags=0; control-client command
        // replies carry flags=1. The unsolicited block must NOT consume a pending
        // command's slot, or every later reply mis-correlates (the copy-mode
        // refresh loop dies when this happens in a multi-pane session).
        let mut c = ProtocolClient::new();
        let id = c.send("display-message -p x").unwrap();
        let _ = c.take_outgoing();
        let events = c
            .feed(b"%begin 1 900 0\nHOOK\n%end 1 900 0\n%begin 1 901 1\nx\n%end 1 901 1\n")
            .unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::CommandComplete {
                id,
                number: 901,
                ok: true,
                output: vec!["x".to_string()],
            }],
            "the flags=1 reply correlates to the sent command; the flags=0 hook block is skipped"
        );
        assert_eq!(
            c.pending_len(),
            0,
            "the command's pending slot is consumed exactly once"
        );
    }

    #[test]
    fn error_reply_is_not_ok() {
        let mut c = ProtocolClient::new();
        let id = c.send("bogus").unwrap();
        let _ = c.take_outgoing();
        let events = c
            .feed(b"%begin 1 9 1\nunknown command\n%error 1 9 1\n")
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
        let events = c.feed(b"%begin 1 2 1\n%end 1 2 1\n").unwrap();
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
            .feed(b"%begin 1 1 1\nA\n%end 1 1 1\n%begin 1 2 1\nB\n%end 1 2 1\n")
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
            .feed(b"%begin 1 1 1\nA\n%end 1 1 1\n%window-add @9\n%begin 1 2 1\nB\n%end 1 2 1\n")
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
    fn unsolicited_flags0_reply_is_skipped_not_fatal() {
        let mut c = ProtocolClient::new();
        // A flags=0 block (a hook's run-shell, or the adopted launch reply) is
        // unsolicited and must be skipped without consuming a pending slot, not
        // close the transport — tmux emits blocks this client did not originate.
        let events = c.feed(b"%begin 1 4 0\n%end 1 4 0\n").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn client_reply_with_no_pending_is_skipped_not_fatal() {
        let mut c = ProtocolClient::new();
        // A flags=1 reply with nothing pending means the FIFO is already desynced;
        // skip it as a safety net rather than closing the transport.
        let events = c.feed(b"%begin 1 5 1\n%end 1 5 1\n").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn command_complete_carries_tmux_number() {
        let mut c = ProtocolClient::new();
        let id = c.send("x").unwrap();
        let _ = c.take_outgoing();
        let events = c.feed(b"%begin 1 42 1\n%end 1 42 1\n").unwrap();
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
    fn malformed_line_outside_block_is_skipped() {
        // A line outside a block that fails to parse as a control event is
        // skipped with a warning rather than closing the transport. This
        // prevents a single malformed notification from blanking all panes.
        let mut c = ProtocolClient::new();
        let events = c.feed(b"hello world\n").unwrap();
        assert!(
            events.is_empty(),
            "malformed notification is silently skipped"
        );
    }

    #[test]
    fn stray_end_outside_block_is_still_fatal() {
        // A stray %end/%error outside a block is a stream desync — fatal.
        let mut c = ProtocolClient::new();
        let err = c.feed(b"%end 1 4 0\n").unwrap_err();
        assert!(matches!(err, TmuxError::Parse(_)));
    }

    #[test]
    fn malformed_body_line_collected_verbatim() {
        let mut c = ProtocolClient::new();
        let id = c.send("x").unwrap();
        let _ = c.take_outgoing();
        let events = c.feed(b"%begin 1 1 1\nhello world\n%end 1 1 1\n").unwrap();
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
    fn adopted_launch_block_skipped_then_first_command_correlates() {
        // A freshly adopted tmux -CC stream emits its launch reply (flags=0, DCS
        // introducer glued to the first %begin) before any client send. It must
        // be skipped, not consume the first real command's pending slot.
        let mut c = ProtocolClient::new();
        let launch = c
            .feed(b"\x1bP1000p%begin 1 1 0\r\n0:work\r\n%end 1 1 0\r\n")
            .expect("feed ok");
        assert!(launch.is_empty(), "the flags=0 launch reply is skipped");

        let id = c.send("list-windows").unwrap();
        let _ = c.take_outgoing();
        let events = c.feed(b"%begin 1 2 1\n0: win\n%end 1 2 1\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::CommandComplete {
                id,
                number: 2,
                ok: true,
                output: vec!["0: win".to_string()],
            }],
            "the first client command correlates after the skipped launch block"
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
        let events = c.feed(b"%begin 1 1 1\nlast-line%end 1 1 1\n").unwrap();
        assert!(events.is_empty());
        assert_eq!(c.pending_len(), 1);
    }

    #[test]
    fn send_effect_queues_raw_then_fence() {
        let mut c = ProtocolClient::new();
        c.send_effect("if-shell -F 1 { display-message a } { display-message b }")
            .unwrap();
        assert_eq!(
            c.take_outgoing(),
            b"if-shell -F 1 { display-message a } { display-message b }\ndisplay-message -p OZMUXFENCE_0\n"
                .to_vec()
        );
    }

    #[test]
    fn effect_drains_all_blocks_until_fence() {
        let mut c = ProtocolClient::new();
        c.send_effect("if-shell -F 1 { a } { b }").unwrap();
        let _ = c.take_outgoing();
        let events = c
            .feed(b"%begin 1 1 1\n%end 1 1 1\n%begin 1 2 1\n%end 1 2 1\n%begin 1 3 1\nOZMUXFENCE_0\n%end 1 3 1\n")
            .unwrap();
        assert!(events.is_empty(), "all relay+fence blocks drained");
        assert_eq!(c.pending_len(), 0);
    }

    #[test]
    fn effect_drain_keeps_following_query_correlated() {
        let mut c = ProtocolClient::new();
        c.send_effect("if-shell -F 1 { a } { b }").unwrap();
        let q = c.send("capture-pane -p -t %1").unwrap();
        let _ = c.take_outgoing();
        let events = c
            .feed(b"%begin 1 1 1\n%end 1 1 1\n%begin 1 2 1\n%end 1 2 1\n%begin 1 3 1\nOZMUXFENCE_0\n%end 1 3 1\n%begin 1 4 1\nSCREEN\n%end 1 4 1\n")
            .unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::CommandComplete {
                id: q,
                number: 4,
                ok: true,
                output: vec!["SCREEN".to_string()],
            }]
        );
    }

    #[test]
    fn effect_drain_passes_notifications_through() {
        let mut c = ProtocolClient::new();
        c.send_effect("select-pane -t %0").unwrap();
        let _ = c.take_outgoing();
        let events = c
            .feed(b"%begin 1 1 1\n%end 1 1 1\n%output %0 hi\n%begin 1 2 1\nOZMUXFENCE_0\n%end 1 2 1\n")
            .unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::Output {
                pane: PaneId(0),
                data: b"hi".to_vec(),
            })]
        );
        assert_eq!(c.pending_len(), 0);
    }

    #[test]
    fn effect_drain_survives_relay_parse_error() {
        let mut c = ProtocolClient::new();
        c.send_effect("nonexistent-xyz").unwrap();
        let _ = c.take_outgoing();
        let events = c
            .feed(b"%begin 1 1 1\nparse error\n%error 1 1 1\n%begin 1 2 1\nOZMUXFENCE_0\n%end 1 2 1\n")
            .unwrap();
        assert!(events.is_empty());
        assert_eq!(c.pending_len(), 0);
    }

    #[test]
    fn effect_drain_ignores_failed_fence_like_error() {
        let mut c = ProtocolClient::new();
        c.send_effect("x").unwrap();
        let _ = c.take_outgoing();
        // An %error whose body coincidentally equals the token must NOT end the drain.
        let events = c
            .feed(b"%begin 1 1 1\nOZMUXFENCE_0\n%error 1 1 1\n")
            .unwrap();
        assert!(events.is_empty());
        assert_eq!(
            c.pending_len(),
            1,
            "drain stays open after a failed fence-like block"
        );
        let events2 = c.feed(b"%begin 1 2 1\nOZMUXFENCE_0\n%end 1 2 1\n").unwrap();
        assert!(events2.is_empty());
        assert_eq!(c.pending_len(), 0);
    }

    #[test]
    fn back_to_back_effects_drain_independently() {
        let mut c = ProtocolClient::new();
        c.send_effect("a").unwrap();
        c.send_effect("b").unwrap();
        let _ = c.take_outgoing();
        let events = c
            .feed(b"%begin 1 1 1\n%end 1 1 1\n%begin 1 2 1\nOZMUXFENCE_0\n%end 1 2 1\n%begin 1 3 1\n%end 1 3 1\n%begin 1 4 1\nOZMUXFENCE_1\n%end 1 4 1\n")
            .unwrap();
        assert!(events.is_empty());
        assert_eq!(c.pending_len(), 0);
    }

    #[test]
    fn effect_drain_backstop_recovers_on_runaway() {
        let mut c = ProtocolClient::new();
        c.send_effect("x").unwrap();
        let _ = c.take_outgoing();
        let mut stream = Vec::new();
        for i in 0..=256u32 {
            let n = i + 10;
            stream.extend_from_slice(format!("%begin 1 {n} 1\nnope\n%end 1 {n} 1\n").as_bytes());
        }
        let events = c
            .feed(&stream)
            .expect("runaway is abandoned locally (logged), not surfaced as a feed() error");
        assert!(events.is_empty(), "drained blocks surface nothing");
        assert_eq!(
            c.pending_len(),
            0,
            "runaway Drain was abandoned and FIFO unblocked"
        );
        let id = c.send("x").unwrap();
        let _ = c.take_outgoing();
        let events = c.feed(b"%begin 1 300 1\nOK\n%end 1 300 1\n").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::CommandComplete {
                id,
                number: 300,
                ok: true,
                output: vec!["OK".to_string()],
            }],
            "FIFO unblocked: subsequent command correlates normally"
        );
    }

    #[test]
    fn relayed_if_shell_keeps_two_pane_reseed_correlated() {
        // The end-to-end regression: a relayed if-shell (2 blocks) before a two-pane
        // reseed must NOT shift the FIFO, so the cursor reply "54 39" stays the cursor
        // reply and never lands on a capture slot.
        let mut c = ProtocolClient::new();
        c.send_effect("if-shell -F 1 { display-message a } { display-message b }")
            .unwrap();
        let cap_p1 = c.send("capture-pane -p -e -t %1").unwrap();
        let cur_p1 = c
            .send("display-message -p -t %1 '#{cursor_x} #{cursor_y}'")
            .unwrap();
        let cap_p2 = c.send("capture-pane -p -e -t %2").unwrap();
        let cur_p2 = c
            .send("display-message -p -t %2 '#{cursor_x} #{cursor_y}'")
            .unwrap();
        let _ = c.take_outgoing();
        let stream = concat!(
            "%begin 1 100 1\n%end 1 100 1\n",
            "%begin 1 101 1\n%end 1 101 1\n",
            "%begin 1 102 1\nOZMUXFENCE_0\n%end 1 102 1\n",
            "%begin 1 103 1\nP1\n%end 1 103 1\n",
            "%begin 1 104 1\n54 39\n%end 1 104 1\n",
            "%begin 1 105 1\nP2\n%end 1 105 1\n",
            "%begin 1 106 1\n7 12\n%end 1 106 1\n",
        );
        let events = c.feed(stream.as_bytes()).unwrap();
        let body = |id: CommandId| {
            events
                .iter()
                .find_map(|e| match e {
                    ClientEvent::CommandComplete { id: i, output, .. } if *i == id => {
                        Some(output.clone())
                    }
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(body(cap_p1), vec!["P1".to_string()]);
        assert_eq!(body(cur_p1), vec!["54 39".to_string()]);
        assert_eq!(body(cap_p2), vec!["P2".to_string()]);
        assert_eq!(body(cur_p2), vec!["7 12".to_string()]);
    }
}
