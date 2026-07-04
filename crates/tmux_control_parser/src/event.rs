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
    /// `%session-renamed <session> <name>`.
    SessionRenamed { session: SessionId, name: String },
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
    /// The session/window/index/pane fields carry `-` for subscription scopes
    /// that do not target them (the window scope `@*` sends `-` for pane; the
    /// session scope sends `-` for all four), so each is `Option`.
    SubscriptionChanged {
        name: String,
        session: Option<SessionId>,
        window: Option<WindowId>,
        window_index: Option<i32>,
        pane: Option<PaneId>,
        value: String,
    },

    /// An unrecognised `%keyword rest` line, kept for forward compatibility.
    Unknown { name: String, rest: String },
}

impl ControlEvent {
    /// Parses a single control-mode line into a [`ControlEvent`].
    pub fn parse(line: &[u8]) -> TmuxResult<Self> {
        if line.iter().all(|b| b.is_ascii_whitespace()) {
            return Err(TmuxError::Empty);
        }
        if line[0] != b'%' {
            return Err(TmuxError::NotControlLine);
        }
        let mut fields = Fields(line);
        let argv = fields.next().ok_or(TmuxError::Empty)?;
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
                session: fields.session("session-renamed")?,
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
            b"%layout-change" => {
                let window = fields.window("layout-change")?;
                let layout = fields.layout("layout-change", "layout")?;
                // NOTE: visible_layout was added in tmux 3.2; older versions
                // send only `%layout-change <window> <layout> <flags>`.
                // Attempt to parse the next token as a layout; if it fails,
                // treat the entire remainder as the flags string and fall back
                // to cloning the main layout so the event stays valid.
                let (visible_layout, flags) = match fields.layout("layout-change", "visible_layout")
                {
                    Ok(vl) => (vl, text(fields.rest(), "flags")?),
                    Err(_) => (
                        layout.clone(),
                        text(fields.rest(), "flags").unwrap_or_default(),
                    ),
                };
                Ok(ControlEvent::LayoutChange {
                    window,
                    layout,
                    visible_layout,
                    flags,
                })
            }
            b"%client-detached" => Ok(ControlEvent::ClientDetached {
                client: fields.required_text("client-detached", "client")?,
            }),
            b"%client-session-changed" => Ok(ControlEvent::ClientSessionChanged {
                client: fields.token("client-session-changed", "client")?,
                session: fields.session("client-session-changed")?,
                name: fields.name("client-session-changed")?,
            }),
            b"%subscription-changed" => {
                let name = fields.token("subscription-changed", "name")?;
                let session = fields.opt_session("subscription-changed")?;
                let window = fields.opt_window("subscription-changed")?;
                let window_index = fields.opt_int_field("subscription-changed", "window_index")?;
                let pane = fields.opt_pane("subscription-changed")?;
                let value = fields.skip_to_colon_value("subscription-changed")?;
                Ok(ControlEvent::SubscriptionChanged {
                    name,
                    session,
                    window,
                    window_index,
                    pane,
                    value,
                })
            }
            b"%output" => Ok(ControlEvent::Output {
                pane: fields.pane("output")?,
                data: unescape_output(fields.rest())?,
            }),
            b"%extended-output" => {
                let pane = fields.pane("extended-output")?;
                let age = fields.int_field("extended-output", "age")?;
                fields.skip_to_colon("extended-output")?;
                let data = unescape_output(fields.rest())?;
                Ok(ControlEvent::ExtendedOutput { pane, age, data })
            }
            _ => {
                let name = text(&argv[1..], "name")?;
                let rest = text(fields.rest(), "rest")?;
                Ok(ControlEvent::Unknown { name, rest })
            }
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
        let time = self.int_field(event, "time")?;
        let number = self.int_field(event, "number")?;
        let flags = self.int_field(event, "flags")?;
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

    fn opt_session(&mut self, event: &'static str) -> TmuxResult<Option<SessionId>> {
        let bytes = self.next().ok_or(TmuxError::MissingField {
            event,
            field: "session",
        })?;
        if bytes == b"-" {
            return Ok(None);
        }
        parse_id(bytes, b'$').map(|s| Some(SessionId(s)))
    }

    fn opt_window(&mut self, event: &'static str) -> TmuxResult<Option<WindowId>> {
        let bytes = self.next().ok_or(TmuxError::MissingField {
            event,
            field: "window",
        })?;
        if bytes == b"-" {
            return Ok(None);
        }
        parse_id(bytes, b'@').map(|w| Some(WindowId(w)))
    }

    fn opt_pane(&mut self, event: &'static str) -> TmuxResult<Option<PaneId>> {
        let bytes = self.next().ok_or(TmuxError::MissingField {
            event,
            field: "pane",
        })?;
        if bytes == b"-" {
            return Ok(None);
        }
        parse_id(bytes, b'%').map(|p| Some(PaneId(p)))
    }

    fn opt_int_field<T: core::str::FromStr>(
        &mut self,
        event: &'static str,
        field: &'static str,
    ) -> TmuxResult<Option<T>> {
        let bytes = self
            .next()
            .ok_or(TmuxError::MissingField { event, field })?;
        if bytes == b"-" {
            return Ok(None);
        }
        int(bytes, field).map(Some)
    }

    fn layout(&mut self, event: &'static str, field: &'static str) -> TmuxResult<WindowLayout> {
        let bytes = self
            .next()
            .ok_or(TmuxError::MissingField { event, field })?;
        WindowLayout::parse(bytes)
    }

    fn token(&mut self, event: &'static str, field: &'static str) -> TmuxResult<String> {
        let bytes = self
            .next()
            .ok_or(TmuxError::MissingField { event, field })?;
        text(bytes, field)
    }

    fn int_field<T: core::str::FromStr>(
        &mut self,
        event: &'static str,
        field: &'static str,
    ) -> TmuxResult<T> {
        let bytes = self
            .next()
            .ok_or(TmuxError::MissingField { event, field })?;
        int(bytes, field)
    }

    fn skip_to_colon(&mut self, event: &'static str) -> TmuxResult<()> {
        loop {
            match self.next() {
                Some(b":") => return Ok(()),
                Some(_) => {}
                None => {
                    return Err(TmuxError::MissingField {
                        event,
                        field: "value",
                    });
                }
            }
        }
    }

    fn skip_to_colon_value(&mut self, event: &'static str) -> TmuxResult<String> {
        self.skip_to_colon(event)?;
        text(self.rest(), "value")
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

/// Decodes `%output` data: `\xxx` octal escapes back to bytes, others verbatim.
fn unescape_output(bytes: &[u8]) -> TmuxResult<Vec<u8>> {
    let invalid = |raw: &[u8]| TmuxError::InvalidOctal {
        raw: String::from_utf8_lossy(raw).into_owned(),
    };
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        match bytes.get(i + 1..i + 4) {
            Some(triple) if triple.iter().all(|&d| matches!(d, b'0'..=b'7')) => {
                let value = u16::from(triple[0] - b'0') * 64
                    + u16::from(triple[1] - b'0') * 8
                    + u16::from(triple[2] - b'0');
                if value > u16::from(u8::MAX) {
                    return Err(invalid(&bytes[i..i + 4]));
                }
                out.push(value as u8);
                i += 4;
            }
            _ => return Err(invalid(&bytes[i..bytes.len().min(i + 4)])),
        }
    }
    Ok(out)
}

/// Decodes `capture-pane -C` octal escapes (`\xxx`) back to bytes, passing
/// any non-octal backslash sequence through verbatim.
///
/// Unlike [`unescape_output`] this never fails: tmux's `-C` escaping is
/// known not to escape backslash itself, so a literal `\` in pane output
/// arrives bare and must not be treated as a protocol error.
pub fn unescape_capture(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        match bytes.get(i + 1..i + 4) {
            Some(triple) if triple.iter().all(|&d| matches!(d, b'0'..=b'7')) => {
                let value = u16::from(triple[0] - b'0') * 64
                    + u16::from(triple[1] - b'0') * 8
                    + u16::from(triple[2] - b'0');
                if value <= u16::from(u8::MAX) {
                    out.push(value as u8);
                    i += 4;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    out
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
            ev(b"%session-renamed $0 renamed"),
            ControlEvent::SessionRenamed {
                session: SessionId(0),
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
    fn layout_change_two_field_older_tmux() {
        // tmux < 3.2 sends %layout-change without a visible_layout field.
        // The parser must not fail; visible_layout falls back to layout.
        let event = ev(b"%layout-change @1 b25f,80x24,0,0,2 *");
        assert!(
            matches!(
                event,
                ControlEvent::LayoutChange {
                    window: WindowId(1),
                    ..
                }
            ),
            "should parse as LayoutChange for window @1"
        );
        if let ControlEvent::LayoutChange {
            layout,
            visible_layout,
            ..
        } = event
        {
            assert_eq!(
                layout, visible_layout,
                "visible_layout falls back to layout"
            );
        }
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
    fn subscription_changed_pane_form_is_all_present() {
        assert_eq!(
            ev(b"%subscription-changed my-sub $1 @2 0 %3 : the-value"),
            ControlEvent::SubscriptionChanged {
                name: "my-sub".to_string(),
                session: Some(SessionId(1)),
                window: Some(WindowId(2)),
                window_index: Some(0),
                pane: Some(PaneId(3)),
                value: "the-value".to_string(),
            }
        );
    }

    #[test]
    fn subscription_changed_window_form_has_dash_pane() {
        assert_eq!(
            ev(b"%subscription-changed wf $1 @2 0 - : *Z"),
            ControlEvent::SubscriptionChanged {
                name: "wf".to_string(),
                session: Some(SessionId(1)),
                window: Some(WindowId(2)),
                window_index: Some(0),
                pane: None,
                value: "*Z".to_string(),
            }
        );
    }

    #[test]
    fn subscription_changed_session_form_is_all_dashes() {
        assert_eq!(
            ev(b"%subscription-changed s - - - - : v"),
            ControlEvent::SubscriptionChanged {
                name: "s".to_string(),
                session: None,
                window: None,
                window_index: None,
                pane: None,
                value: "v".to_string(),
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

    #[test]
    fn unescape_capture_decodes_octal_triples() {
        assert_eq!(unescape_capture(b"abc\\015\\012def"), b"abc\r\ndef".to_vec());
    }

    #[test]
    fn unescape_capture_passes_lone_backslash_verbatim() {
        // NOTE: capture-pane -C escapes control chars but NOT backslash itself,
        // so a literal backslash followed by non-octal must survive unchanged.
        assert_eq!(unescape_capture(b"a\\x9"), b"a\\x9".to_vec());
        assert_eq!(unescape_capture(b"tail\\"), b"tail\\".to_vec());
    }

    #[test]
    fn unescape_capture_passes_out_of_range_octal_verbatim() {
        assert_eq!(unescape_capture(b"\\400"), b"\\400".to_vec());
    }
}
