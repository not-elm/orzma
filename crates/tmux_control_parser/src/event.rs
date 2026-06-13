//! Parsed control-mode events and the typed entity ids they carry.

use crate::{TmuxError, error::TmuxResult, layout::WindowLayout};

/// A tmux pane id (`%N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u32);

/// A tmux window id (`@N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u32);

/// A tmux session id (`$N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u32);

/// A single parsed control-mode line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    /// `%begin <time> <number> <flags>` — start of a command output block.
    Begin { time: u64, number: u32, flags: u32 },
    /// `%end <time> <number> <flags>` — successful end of a command output block.
    End { time: u64, number: u32, flags: u32 },
    /// `%error <time> <number> <flags>` — failed end of a command output block.
    Error { time: u64, number: u32, flags: u32 },

    /// `%output <pane> <data>` — pane output, octal-decoded to raw bytes.
    Output { pane: PaneId, data: Vec<u8> },
    /// `%extended-output <pane> <age> ... : <data>` — output with buffering age (ms).
    ExtendedOutput {
        pane: PaneId,
        age: u64,
        data: Vec<u8>,
    },

    /// `%window-add <window>`.
    WindowAdd { window: WindowId },
    /// `%window-close <window>`.
    WindowClose { window: WindowId },
    /// `%window-renamed <window> <name>`.
    WindowRenamed { window: WindowId, name: String },
    /// `%window-pane-changed <window> <pane>`.
    WindowPaneChanged { window: WindowId, pane: PaneId },
    /// `%unlinked-window-add <window>`.
    UnlinkedWindowAdd { window: WindowId },
    /// `%unlinked-window-close <window>`.
    UnlinkedWindowClose { window: WindowId },
    /// `%unlinked-window-renamed <window>` (no name arg in tmux 3.6a).
    UnlinkedWindowRenamed { window: WindowId },
    /// `%pane-mode-changed <pane>`.
    PaneModeChanged { pane: PaneId },

    /// `%session-changed <session> <name>`.
    SessionChanged { session: SessionId, name: String },
    /// `%session-renamed <name>`.
    SessionRenamed { name: String },
    /// `%session-window-changed <session> <window>`.
    SessionWindowChanged {
        session: SessionId,
        window: WindowId,
    },
    /// `%sessions-changed`.
    SessionsChanged,

    /// `%client-detached <client>`.
    ClientDetached { client: String },
    /// `%client-session-changed <client> <session> <name>`.
    ClientSessionChanged {
        client: String,
        session: SessionId,
        name: String,
    },
    /// `%layout-change <window> <layout> <visible-layout> <flags>`.
    LayoutChange {
        window: WindowId,
        layout: WindowLayout,
        visible_layout: WindowLayout,
        flags: String,
    },
    /// `%continue <pane>`.
    Continue { pane: PaneId },
    /// `%pause <pane>`.
    Pause { pane: PaneId },
    /// `%exit [reason]`.
    Exit { reason: Option<String> },
    /// `%message <message>`.
    Message { message: String },
    /// `%config-error <message>`.
    ConfigError { message: String },
    /// `%paste-buffer-changed <name>`.
    PasteBufferChanged { name: String },
    /// `%paste-buffer-deleted <name>`.
    PasteBufferDeleted { name: String },
    /// `%subscription-changed <name> <session> <window> <index> <pane> ... : <value>`.
    SubscriptionChanged {
        name: String,
        session: SessionId,
        window: WindowId,
        window_index: i32,
        pane: PaneId,
        value: String,
    },

    /// An unrecognised `%keyword rest` line, kept for forward compatibility.
    Unknown { name: String, rest: String },
}

impl ControlEvent {
    /// Parses a single control-mode line into a [`ControlEvent`].
    pub fn parse(line: &[u8]) -> TmuxResult<Self> {
        let mut fields = Fields(line);
        let argv = fields.next().ok_or(TmuxError::NotControlLine)?;
        match argv {
            b"%begin" => fields.parse_guard("begin", |time, number, flags| ControlEvent::Begin {
                time,
                number,
                flags,
            }),
            b"%end" => fields.parse_guard("end", |time, number, flags| ControlEvent::End {
                time,
                number,
                flags,
            }),
            b"%error" => fields.parse_guard("error", |time, number, flags| ControlEvent::Error {
                time,
                number,
                flags,
            }),
            b"%window-add" => Ok(ControlEvent::WindowAdd {
                window: fields.window("window-add")?,
            }),
            b"%window-close" => Ok(ControlEvent::WindowClose {
                window: fields.window("window-close")?,
            }),
            b"%window-renamed" => Ok(ControlEvent::WindowRenamed {
                window: fields.window("window-renamed")?,
                name: fields.name("window-renamed")?,
            }),
            b"%window-pane-changed" => Ok(ControlEvent::WindowPaneChanged {
                window: fields.window("window-pane-changed")?,
                pane: fields.pane("window-pane-changed")?,
            }),
            b"%unlinked-window-add" => Ok(ControlEvent::UnlinkedWindowAdd {
                window: fields.window("unlinked-window-add")?,
            }),
            b"%unlinked-window-close" => Ok(ControlEvent::UnlinkedWindowClose {
                window: fields.window("unlinked-window-close")?,
            }),
            b"%unlinked-window-renamed" => Ok(ControlEvent::UnlinkedWindowRenamed {
                window: fields.window("unlinked-window-renamed")?,
            }),
            b"%session-changed" => Ok(ControlEvent::SessionChanged {
                session: fields.session("session-changed")?,
                name: fields.name("session-changed")?,
            }),
            b"%session-renamed" => Ok(ControlEvent::SessionRenamed {
                name: fields.name("session-renamed")?,
            }),
            b"%session-window-changed" => Ok(ControlEvent::SessionWindowChanged {
                session: fields.session("session-window-changed")?,
                window: fields.window("session-window-changed")?,
            }),
            b"%sessions-changed" => Ok(ControlEvent::SessionsChanged),
            b"%pane-mode-changed" => Ok(ControlEvent::PaneModeChanged {
                pane: fields.pane("pane-mode-changed")?,
            }),
            b"%continue" => Ok(ControlEvent::Continue {
                pane: fields.pane("continue")?,
            }),
            b"%pause" => Ok(ControlEvent::Pause {
                pane: fields.pane("pause")?,
            }),
            b"%exit" => {
                let rest = fields.rest();
                let reason = if rest.is_empty() {
                    None
                } else {
                    Some(text(rest, "reason")?)
                };
                Ok(ControlEvent::Exit { reason })
            }
            b"%message" => Ok(ControlEvent::Message {
                message: fields.required_text("message", "message")?,
            }),
            b"%config-error" => Ok(ControlEvent::ConfigError {
                message: fields.required_text("config-error", "message")?,
            }),
            b"%paste-buffer-changed" => Ok(ControlEvent::PasteBufferChanged {
                name: fields.name("paste-buffer-changed")?,
            }),
            b"%paste-buffer-deleted" => Ok(ControlEvent::PasteBufferDeleted {
                name: fields.name("paste-buffer-deleted")?,
            }),
            b"%layout-change" => Ok(ControlEvent::LayoutChange {
                window: fields.window("layout-change")?,
                layout: fields.layout("layout-change", "layout")?,
                visible_layout: fields.layout("layout-change", "visible_layout")?,
                flags: text(fields.rest(), "flags")?,
            }),
            _ => todo!("ControlEvent::parse"),
        }
    }
}

struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    /// Next space-delimited field, or None when exhausted.
    fn next(&mut self) -> Option<&'a [u8]> {
        if self.0.is_empty() {
            return None;
        }
        match self.0.iter().position(|&b| b == b' ') {
            Some(i) => {
                let (field, rest) = self.0.split_at(i);
                self.0 = &rest[1..]; // drop the single separating space
                Some(field)
            }
            None => {
                let field = self.0;
                self.0 = &[];
                Some(field)
            }
        }
    }

    /// Everything left, verbatim — the trailing name/message/value.
    fn rest(&self) -> &'a [u8] {
        self.0
    }

    fn parse_guard(
        &mut self,
        event: &'static str,
        build: fn(u64, u32, u32) -> ControlEvent,
    ) -> TmuxResult<ControlEvent> {
        let time = int(
            self.next().ok_or(TmuxError::MissingField {
                event,
                field: "time",
            })?,
            "time",
        )?;
        let number = int(
            self.next().ok_or(TmuxError::MissingField {
                event,
                field: "number",
            })?,
            "number",
        )?;
        let flags = int(
            self.next().ok_or(TmuxError::MissingField {
                event,
                field: "flags",
            })?,
            "flags",
        )?;
        Ok(build(time, number, flags))
    }

    fn window(&mut self, event: &'static str) -> TmuxResult<WindowId> {
        let bytes = self.next().ok_or(TmuxError::MissingField {
            event,
            field: "window",
        })?;
        parse_id(bytes, b'@').map(WindowId)
    }

    fn pane(&mut self, event: &'static str) -> TmuxResult<PaneId> {
        let bytes = self.next().ok_or(TmuxError::MissingField {
            event,
            field: "pane",
        })?;
        parse_id(bytes, b'%').map(PaneId)
    }

    fn session(&mut self, event: &'static str) -> TmuxResult<SessionId> {
        let bytes = self.next().ok_or(TmuxError::MissingField {
            event,
            field: "session",
        })?;
        parse_id(bytes, b'$').map(SessionId)
    }

    fn layout(&mut self, event: &'static str, field: &'static str) -> TmuxResult<WindowLayout> {
        let bytes = self
            .next()
            .ok_or(TmuxError::MissingField { event, field })?;
        WindowLayout::parse(bytes)
    }

    fn name(&self, event: &'static str) -> TmuxResult<String> {
        self.required_text(event, "name")
    }

    fn required_text(&self, event: &'static str, field: &'static str) -> TmuxResult<String> {
        if self.0.is_empty() {
            return Err(TmuxError::MissingField { event, field });
        }
        text(self.rest(), field)
    }
}

/// Parses an ASCII integer field into `T`, mapping failures to [`TmuxError::InvalidInt`].
fn int<T: core::str::FromStr>(bytes: &[u8], field: &'static str) -> TmuxResult<T> {
    core::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| TmuxError::InvalidInt {
            field,
            raw: String::from_utf8_lossy(bytes).into_owned(),
        })
}

/// Parses an entity id (`@3` / `%7` / `$1`) into its numeric part, checking the prefix.
fn parse_id(bytes: &[u8], prefix: u8) -> TmuxResult<u32> {
    let err = || TmuxError::InvalidId {
        raw: String::from_utf8_lossy(bytes).into_owned(),
        expected: prefix as char,
    };
    match bytes.split_first() {
        Some((&p, rest)) if p == prefix => core::str::from_utf8(rest)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(err),
        _ => Err(err()),
    }
}

/// Decodes a byte slice tmux guarantees to be UTF-8 (name/layout/message) into a [`String`].
fn text(bytes: &[u8], field: &'static str) -> TmuxResult<String> {
    core::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| TmuxError::InvalidUtf8 { field })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TmuxError;
    use crate::layout::{Cell, CellDims};

    fn ev(line: &[u8]) -> ControlEvent {
        ControlEvent::parse(line).expect("line should parse")
    }

    #[test]
    fn command_block_guards() {
        assert_eq!(
            ev(b"%begin 1363006971 2 1"),
            ControlEvent::Begin {
                time: 1363006971,
                number: 2,
                flags: 1
            }
        );
        assert_eq!(
            ev(b"%end 1363006971 2 1"),
            ControlEvent::End {
                time: 1363006971,
                number: 2,
                flags: 1
            }
        );
        assert_eq!(
            ev(b"%error 1363006971 2 1"),
            ControlEvent::Error {
                time: 1363006971,
                number: 2,
                flags: 1
            }
        );
    }

    #[test]
    fn window_lifecycle() {
        assert_eq!(
            ev(b"%window-add @1"),
            ControlEvent::WindowAdd {
                window: WindowId(1)
            }
        );
        assert_eq!(
            ev(b"%window-close @2"),
            ControlEvent::WindowClose {
                window: WindowId(2)
            }
        );
        assert_eq!(
            ev(b"%window-renamed @3 my window"),
            ControlEvent::WindowRenamed {
                window: WindowId(3),
                name: "my window".to_string()
            }
        );
        assert_eq!(
            ev(b"%window-pane-changed @3 %7"),
            ControlEvent::WindowPaneChanged {
                window: WindowId(3),
                pane: PaneId(7)
            }
        );
    }

    #[test]
    fn unlinked_window() {
        assert_eq!(
            ev(b"%unlinked-window-add @9"),
            ControlEvent::UnlinkedWindowAdd {
                window: WindowId(9)
            }
        );
        assert_eq!(
            ev(b"%unlinked-window-close @9"),
            ControlEvent::UnlinkedWindowClose {
                window: WindowId(9)
            }
        );
        assert_eq!(
            ev(b"%unlinked-window-renamed @9"),
            ControlEvent::UnlinkedWindowRenamed {
                window: WindowId(9)
            }
        );
    }

    #[test]
    fn session_notifications() {
        assert_eq!(
            ev(b"%session-changed $1 main"),
            ControlEvent::SessionChanged {
                session: SessionId(1),
                name: "main".to_string()
            }
        );
        assert_eq!(
            ev(b"%session-renamed renamed"),
            ControlEvent::SessionRenamed {
                name: "renamed".to_string()
            }
        );
        assert_eq!(
            ev(b"%session-window-changed $1 @4"),
            ControlEvent::SessionWindowChanged {
                session: SessionId(1),
                window: WindowId(4)
            }
        );
        assert_eq!(ev(b"%sessions-changed"), ControlEvent::SessionsChanged);
    }

    #[test]
    fn pane_mode_continue_pause() {
        assert_eq!(
            ev(b"%pane-mode-changed %5"),
            ControlEvent::PaneModeChanged { pane: PaneId(5) }
        );
        assert_eq!(
            ev(b"%continue %5"),
            ControlEvent::Continue { pane: PaneId(5) }
        );
        assert_eq!(ev(b"%pause %5"), ControlEvent::Pause { pane: PaneId(5) });
    }

    #[test]
    fn client_notifications() {
        assert_eq!(
            ev(b"%client-detached /dev/ttys003"),
            ControlEvent::ClientDetached {
                client: "/dev/ttys003".to_string()
            }
        );
        assert_eq!(
            ev(b"%client-session-changed /dev/ttys003 $2 work"),
            ControlEvent::ClientSessionChanged {
                client: "/dev/ttys003".to_string(),
                session: SessionId(2),
                name: "work".to_string(),
            }
        );
    }

    #[test]
    fn layout_change() {
        assert_eq!(
            ev(b"%layout-change @1 b25f,80x24,0,0,2 b25f,80x24,0,0,2 *"),
            ControlEvent::LayoutChange {
                window: WindowId(1),
                layout: WindowLayout {
                    checksum: 0xb25f,
                    root: Cell::Leaf {
                        dims: CellDims {
                            width: 80,
                            height: 24,
                            xoff: 0,
                            yoff: 0,
                        },
                        pane_id: Some(2),
                    },
                },
                visible_layout: WindowLayout {
                    checksum: 0xb25f,
                    root: Cell::Leaf {
                        dims: CellDims {
                            width: 80,
                            height: 24,
                            xoff: 0,
                            yoff: 0,
                        },
                        pane_id: Some(2),
                    },
                },
                flags: "*".to_string(),
            }
        );
    }

    #[test]
    fn exit_with_and_without_reason() {
        assert_eq!(ev(b"%exit"), ControlEvent::Exit { reason: None });
        assert_eq!(
            ev(b"%exit server exited unexpectedly"),
            ControlEvent::Exit {
                reason: Some("server exited unexpectedly".to_string())
            }
        );
    }

    #[test]
    fn message_and_config_error() {
        assert_eq!(
            ev(b"%message hello there"),
            ControlEvent::Message {
                message: "hello there".to_string()
            }
        );
        assert_eq!(
            ev(b"%config-error /home/u/.tmux.conf:3: unknown command"),
            ControlEvent::ConfigError {
                message: "/home/u/.tmux.conf:3: unknown command".to_string()
            }
        );
    }

    #[test]
    fn paste_buffer_notifications() {
        assert_eq!(
            ev(b"%paste-buffer-changed buffer0"),
            ControlEvent::PasteBufferChanged {
                name: "buffer0".to_string()
            }
        );
        assert_eq!(
            ev(b"%paste-buffer-deleted buffer0"),
            ControlEvent::PasteBufferDeleted {
                name: "buffer0".to_string()
            }
        );
    }

    #[test]
    fn subscription_changed_documented_form() {
        // TODO: verify against a live session — for session-scoped subscriptions the
        // session/window/index/pane fields may be empty and require Option<>.
        assert_eq!(
            ev(b"%subscription-changed my-sub $1 @2 0 %3 : the-value"),
            ControlEvent::SubscriptionChanged {
                name: "my-sub".to_string(),
                session: SessionId(1),
                window: WindowId(2),
                window_index: 0,
                pane: PaneId(3),
                value: "the-value".to_string(),
            }
        );
    }

    #[test]
    fn output_octal_round_trip() {
        assert_eq!(
            ControlEvent::parse(b"%output %1 abc\\015\\012def"),
            Ok(ControlEvent::Output {
                pane: PaneId(1),
                data: b"abc\r\ndef".to_vec()
            })
        );
    }

    #[test]
    fn output_escaped_backslash() {
        assert_eq!(
            ControlEvent::parse(b"%output %1 a\\134b"),
            Ok(ControlEvent::Output {
                pane: PaneId(1),
                data: b"a\\b".to_vec()
            })
        );
    }

    #[test]
    fn output_value_keeps_spaces() {
        assert_eq!(
            ControlEvent::parse(b"%output %1 echo hello world"),
            Ok(ControlEvent::Output {
                pane: PaneId(1),
                data: b"echo hello world".to_vec()
            })
        );
    }

    #[test]
    fn output_empty_value() {
        assert_eq!(
            ControlEvent::parse(b"%output %1 "),
            Ok(ControlEvent::Output {
                pane: PaneId(1),
                data: Vec::new()
            })
        );
    }

    #[test]
    fn output_passes_through_high_bytes() {
        let mut line = b"%output %2 caf".to_vec();
        line.extend_from_slice(&[0xC3, 0xA9]);
        assert_eq!(
            ControlEvent::parse(&line),
            Ok(ControlEvent::Output {
                pane: PaneId(2),
                data: vec![b'c', b'a', b'f', 0xC3, 0xA9]
            })
        );
    }

    #[test]
    fn output_octal_escape_to_control_byte() {
        assert_eq!(
            ControlEvent::parse(b"%output %1 \\033[31m"),
            Ok(ControlEvent::Output {
                pane: PaneId(1),
                data: vec![0x1b, b'[', b'3', b'1', b'm']
            })
        );
    }

    #[test]
    fn extended_output_basic() {
        assert_eq!(
            ControlEvent::parse(b"%extended-output %1 250 : abc\\015"),
            Ok(ControlEvent::ExtendedOutput {
                pane: PaneId(1),
                age: 250,
                data: b"abc\r".to_vec()
            })
        );
    }

    #[test]
    fn extended_output_skips_future_args() {
        assert_eq!(
            ControlEvent::parse(b"%extended-output %1 0 reserved1 reserved2 : data"),
            Ok(ControlEvent::ExtendedOutput {
                pane: PaneId(1),
                age: 0,
                data: b"data".to_vec()
            })
        );
    }

    #[test]
    fn output_rejects_short_octal() {
        assert!(matches!(
            ControlEvent::parse(b"%output %1 ab\\01"),
            Err(TmuxError::InvalidOctal { .. })
        ));
    }

    #[test]
    fn output_rejects_non_octal_digit() {
        assert!(matches!(
            ControlEvent::parse(b"%output %1 \\1a2"),
            Err(TmuxError::InvalidOctal { .. })
        ));
    }

    #[test]
    fn output_rejects_overflow_octal() {
        assert!(matches!(
            ControlEvent::parse(b"%output %1 \\400"),
            Err(TmuxError::InvalidOctal { .. })
        ));
    }

    #[test]
    fn output_rejects_trailing_backslash() {
        assert!(matches!(
            ControlEvent::parse(b"%output %1 abc\\"),
            Err(TmuxError::InvalidOctal { .. })
        ));
    }

    #[test]
    fn rejects_non_control_line() {
        assert!(matches!(
            ControlEvent::parse(b"hello world"),
            Err(TmuxError::NotControlLine)
        ));
    }

    #[test]
    fn rejects_empty_line() {
        assert!(matches!(ControlEvent::parse(b""), Err(TmuxError::Empty)));
    }

    #[test]
    fn rejects_id_without_prefix() {
        assert!(matches!(
            ControlEvent::parse(b"%window-add 3"),
            Err(TmuxError::InvalidId { expected: '@', .. })
        ));
    }

    #[test]
    fn rejects_id_with_wrong_prefix() {
        assert!(matches!(
            ControlEvent::parse(b"%window-add %3"),
            Err(TmuxError::InvalidId { expected: '@', .. })
        ));
    }

    #[test]
    fn rejects_non_numeric_id() {
        assert!(matches!(
            ControlEvent::parse(b"%window-add @x"),
            Err(TmuxError::InvalidId { expected: '@', .. })
        ));
    }

    #[test]
    fn rejects_missing_field() {
        assert!(matches!(
            ControlEvent::parse(b"%window-renamed @3"),
            Err(TmuxError::MissingField { .. })
        ));
    }

    #[test]
    fn rejects_bad_integer_in_guard() {
        assert!(matches!(
            ControlEvent::parse(b"%begin notanumber 2 1"),
            Err(TmuxError::InvalidInt { .. })
        ));
    }

    #[test]
    fn unknown_notification_maps_to_unknown() {
        assert_eq!(
            ControlEvent::parse(b"%future-thing some args"),
            Ok(ControlEvent::Unknown {
                name: "future-thing".to_string(),
                rest: "some args".to_string(),
            })
        );
    }
}
