//! Stateful assembler that groups `%begin`..`%end`/`%error` blocks into replies
//! and passes standalone notifications through.

use crate::error::{TmuxError, TmuxResult};
use crate::event::ControlEvent;

/// A higher-level frame emitted by [`BlockAssembler`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A completed command output block.
    Reply {
        /// The command number shared by the matching `%begin`/`%end`/`%error`.
        number: u32,
        /// The `%begin` flags bitmask, verbatim (taken from the opening
        /// `%begin`). tmux populates bit 0 for control-client command replies;
        /// the protocol layer owns that interpretation (see the consumer's
        /// `is_client_command_reply`).
        flags: u32,
        /// `true` if closed by `%end`, `false` if closed by `%error`.
        ok: bool,
        /// Reply body lines, verbatim, in order.
        body: Vec<String>,
    },
    /// A standalone notification that occurred outside any block.
    Notification(ControlEvent),
}

/// Groups raw control-mode lines into [`Frame`]s, tracking the active block.
#[derive(Debug, Default, Clone)]
pub struct BlockAssembler {
    open: Option<OpenBlock>,
}

impl BlockAssembler {
    /// Returns a new assembler with no block open.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one raw line, returning a completed [`Frame`] when one is ready.
    ///
    /// Outside a block, a line is a standalone notification and `%begin` opens a
    /// block. Inside a block, every line is collected verbatim until the
    /// `%end`/`%error` whose command number matches the open `%begin` — a body
    /// line is never reparsed as a notification.
    pub fn feed(&mut self, line: &[u8]) -> TmuxResult<Option<Frame>> {
        match self.open.take() {
            Some(mut block) => {
                let closed_ok = match ControlEvent::parse(line) {
                    Ok(ControlEvent::End { number, .. }) if number == block.number => Some(true),
                    Ok(ControlEvent::Error { number, .. }) if number == block.number => Some(false),
                    _ => None,
                };
                match closed_ok {
                    Some(ok) => Ok(Some(Frame::Reply {
                        number: block.number,
                        flags: block.flags,
                        ok,
                        body: block.body,
                    })),
                    None => {
                        block.body.push(String::from_utf8_lossy(line).into_owned());
                        self.open = Some(block);
                        Ok(None)
                    }
                }
            }
            None => match ControlEvent::parse(line)? {
                ControlEvent::Begin { number, flags, .. } => {
                    self.open = Some(OpenBlock {
                        number,
                        flags,
                        body: Vec::new(),
                    });
                    Ok(None)
                }
                ControlEvent::End { number, .. } | ControlEvent::Error { number, .. } => {
                    Err(TmuxError::UnexpectedEnd { number })
                }
                event => Ok(Some(Frame::Notification(event))),
            },
        }
    }
}

#[derive(Debug, Clone)]
struct OpenBlock {
    number: u32,
    flags: u32,
    body: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TmuxError;
    use crate::event::{PaneId, SessionId, WindowId};

    fn feed_all(lines: &[&[u8]]) -> Vec<Frame> {
        let mut asm = BlockAssembler::new();
        let mut frames = Vec::new();
        for line in lines {
            if let Some(frame) = asm.feed(line).expect("feed should not error") {
                frames.push(frame);
            }
        }
        frames
    }

    #[test]
    fn empty_reply_block() {
        let frames = feed_all(&[b"%begin 1 7 1", b"%end 1 7 1"]);
        assert_eq!(
            frames,
            vec![Frame::Reply {
                number: 7,
                flags: 1,
                ok: true,
                body: Vec::new()
            }]
        );
    }

    #[test]
    fn reply_carries_begin_flags() {
        // flags=0 marks the adopted-stream launch reply or unsolicited internal
        // output (a hook's run-shell); flags=1 a control-client command reply.
        let zero = feed_all(&[b"%begin 1 42 0", b"%end 1 42 0"]);
        assert_eq!(
            zero,
            vec![Frame::Reply {
                number: 42,
                flags: 0,
                ok: true,
                body: Vec::new()
            }]
        );
    }

    #[test]
    fn multi_line_reply_body() {
        let frames = feed_all(&[
            b"%begin 1 8 1",
            b"0: ksh* (1 panes) [80x24]",
            b"1: bash- (1 panes) [80x24]",
            b"%end 1 8 1",
        ]);
        assert_eq!(
            frames,
            vec![Frame::Reply {
                number: 8,
                flags: 1,
                ok: true,
                body: vec![
                    "0: ksh* (1 panes) [80x24]".to_string(),
                    "1: bash- (1 panes) [80x24]".to_string(),
                ],
            }]
        );
    }

    #[test]
    fn error_reply_block() {
        let frames = feed_all(&[b"%begin 1 9 1", b"unknown command: bogus", b"%error 1 9 1"]);
        assert_eq!(
            frames,
            vec![Frame::Reply {
                number: 9,
                flags: 1,
                ok: false,
                body: vec!["unknown command: bogus".to_string()],
            }]
        );
    }

    #[test]
    fn reply_body_line_starting_with_percent_stays_body() {
        let frames = feed_all(&[
            b"%begin 1 10 1",
            b"%not-a-notification literally text",
            b"%end 1 10 1",
        ]);
        assert_eq!(
            frames,
            vec![Frame::Reply {
                number: 10,
                flags: 1,
                ok: true,
                body: vec!["%not-a-notification literally text".to_string()],
            }]
        );
    }

    #[test]
    fn stray_end_with_wrong_number_stays_body() {
        let frames = feed_all(&[b"%begin 1 11 1", b"%end 1 999 1", b"%end 1 11 1"]);
        assert_eq!(
            frames,
            vec![Frame::Reply {
                number: 11,
                flags: 1,
                ok: true,
                body: vec!["%end 1 999 1".to_string()],
            }]
        );
    }

    #[test]
    fn standalone_notification_passes_through() {
        let frames = feed_all(&[b"%window-add @1"]);
        assert_eq!(
            frames,
            vec![Frame::Notification(ControlEvent::WindowAdd {
                window: WindowId(1)
            })]
        );
    }

    #[test]
    fn notifications_interleaved_with_blocks() {
        let frames = feed_all(&[
            b"%output %1 hi",
            b"%begin 1 12 1",
            b"reply",
            b"%end 1 12 1",
            b"%window-close @4",
        ]);
        assert_eq!(
            frames,
            vec![
                Frame::Notification(ControlEvent::Output {
                    pane: PaneId(1),
                    data: b"hi".to_vec()
                }),
                Frame::Reply {
                    number: 12,
                    flags: 1,
                    ok: true,
                    body: vec!["reply".to_string()]
                },
                Frame::Notification(ControlEvent::WindowClose {
                    window: WindowId(4)
                }),
            ]
        );
    }

    #[test]
    fn mid_block_feed_returns_none() {
        let mut asm = BlockAssembler::new();
        assert_eq!(asm.feed(b"%begin 1 13 1"), Ok(None));
        assert_eq!(asm.feed(b"line"), Ok(None));
        assert_eq!(
            asm.feed(b"%end 1 13 1"),
            Ok(Some(Frame::Reply {
                number: 13,
                flags: 1,
                ok: true,
                body: vec!["line".to_string()]
            }))
        );
    }

    #[test]
    fn end_without_open_block_errors() {
        let mut asm = BlockAssembler::new();
        assert!(matches!(
            asm.feed(b"%end 1 14 1"),
            Err(TmuxError::UnexpectedEnd { number: 14 })
        ));
    }

    #[test]
    fn startup_transcript_produces_expected_frames() {
        let transcript: &[&[u8]] = &[
            b"%begin 1363006971 1 1",
            b"%end 1363006971 1 1",
            b"%session-changed $0 0",
            b"%window-add @0",
            b"%output %0 \\033]0;ksh\\007$ ",
            b"%window-renamed @0 ksh",
        ];

        let frames = feed_all(transcript);

        assert_eq!(
            frames,
            vec![
                Frame::Reply {
                    number: 1,
                    flags: 1,
                    ok: true,
                    body: Vec::new()
                },
                Frame::Notification(ControlEvent::SessionChanged {
                    session: SessionId(0),
                    name: "0".to_string(),
                }),
                Frame::Notification(ControlEvent::WindowAdd {
                    window: WindowId(0)
                }),
                Frame::Notification(ControlEvent::Output {
                    pane: PaneId(0),
                    data: b"\x1b]0;ksh\x07$ ".to_vec(),
                }),
                Frame::Notification(ControlEvent::WindowRenamed {
                    window: WindowId(0),
                    name: "ksh".to_string(),
                }),
            ]
        );
    }
}
