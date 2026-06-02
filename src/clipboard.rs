//! Clipboard Bevy Resource. Wraps `arboard::Clipboard` so the rest of
//! the GUI can read and write text without holding the cross-platform
//! handle directly. `build_paste_bytes` is the pure helper that turns
//! clipboard text into the byte stream forwarded to the PTY.
//! `ClipboardActionPlugin` wires the `CopyToClipboardActionEvent` /
//! `PasteFromClipboardActionEvent` observers that bridge `Action::Copy` /
//! `Action::Paste` to the focused terminal.

use bevy::app::{App, Plugin};
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::observer::On;
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Query, ResMut};
use bevy_terminal::{PtyHandle, TerminalHandle};

/// Resource wrapping a lazily-initialized `arboard::Clipboard`.
///
/// `arboard::Clipboard::new()` can fail when no display is available
/// (e.g. headless CI). In that case the inner `Option` stays `None`
/// and every `write` call becomes a no-op (logged at debug level once
/// at init, then silently dropped). Copy-mode UI keeps working — the
/// user can still see the selection — but `y` does not modify the
/// host clipboard.
#[derive(Resource)]
pub struct Clipboard(Option<arboard::Clipboard>);

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    pub fn new() -> Self {
        match arboard::Clipboard::new() {
            Ok(cb) => Self(Some(cb)),
            Err(e) => {
                tracing::warn!(
                    target: "ozmux_gui::clipboard",
                    error = ?e,
                    "arboard init failed; clipboard writes will no-op",
                );
                Self(None)
            }
        }
    }

    /// Writes `text` to the system clipboard. No-op when arboard is
    /// unavailable. Failures are logged at warn but never propagated —
    /// copy mode must not panic on a clipboard failure.
    pub fn write(&mut self, text: String) {
        let Some(cb) = self.0.as_mut() else {
            tracing::debug!(
                target: "ozmux_gui::clipboard",
                "clipboard write skipped: arboard unavailable",
            );
            return;
        };
        if let Err(e) = cb.set_text(text) {
            tracing::warn!(
                target: "ozmux_gui::clipboard",
                error = ?e,
                "clipboard write failed",
            );
        }
    }

    /// Reads text from the system clipboard. Returns `None` when
    /// `arboard` is unavailable (headless) or when the clipboard does
    /// not currently hold UTF-8 text. Empty strings are passed through
    /// as `Some(String::new())`; the caller is responsible for treating
    /// empty as a no-op.
    ///
    /// arboard's behavior on an empty clipboard is platform-dependent —
    /// some backends return `Err(ContentNotAvailable)`, others return
    /// `Ok("")`. Both shapes are handled here (the `Err` path returns
    /// `None`, the `Ok("")` path returns `Some("")`); either way the
    /// caller's `text.is_empty()` check at the dispatcher swallows it
    /// without reaching the PTY.
    pub fn read(&mut self) -> Option<String> {
        let Some(cb) = self.0.as_mut() else {
            tracing::debug!(
                target: "ozmux_gui::clipboard",
                "clipboard read skipped: arboard unavailable",
            );
            return None;
        };
        match cb.get_text() {
            Ok(text) => Some(text),
            Err(arboard::Error::ContentNotAvailable) => {
                tracing::debug!(
                    target: "ozmux_gui::clipboard",
                    "clipboard read: nothing available (empty / non-text)",
                );
                None
            }
            Err(err) => {
                tracing::warn!(
                    target: "ozmux_gui::clipboard",
                    error = ?err,
                    "clipboard read failed",
                );
                None
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn is_available_for_test(&self) -> bool {
        self.0.is_some()
    }
}

/// Constructs the byte sequence that `TerminalHandle::write` should
/// send to the PTY for a paste of `text`.
///
/// - `bracketed = true`: strips every occurrence of the four
///   bracketed-paste marker forms — 7-bit `\x1b[200~` / `\x1b[201~`
///   and 8-bit C1 `\x9b200~` / `\x9b201~` — in a fixed-point loop,
///   then wraps the sanitized body in `\x1b[200~` ... `\x1b[201~`.
///   The loop is required because removing one marker can re-expose
///   another (e.g. `\x1b[\x1b[201~201~` → `\x1b[201~`). Closes the
///   kitty-CVE class documented in kitty commit 668f6fa and
///   Alacritty issue #800.
/// - `bracketed = false`: walks `text` once and normalizes line
///   endings so shells receive a `\r` for each line. `\r\n` collapses
///   to `\r`, lone `\n` becomes `\r`, and existing `\r` bytes pass
///   through unchanged. Matches the xterm / iTerm2 paste convention.
pub(crate) fn build_paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut body = text.to_owned();
        loop {
            let next = body
                .replace("\x1b[200~", "")
                .replace("\x1b[201~", "")
                .replace("\u{9b}200~", "")
                .replace("\u{9b}201~", "");
            if next == body {
                break;
            }
            body = next;
        }
        let mut out = Vec::with_capacity(body.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        let mut out = Vec::with_capacity(text.len());
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'\r' {
                out.push(b'\r');
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 2;
                    continue;
                }
            } else if b == b'\n' {
                out.push(b'\r');
            } else {
                out.push(b);
            }
            i += 1;
        }
        out
    }
}

/// Request to copy the focused terminal's current selection to the
/// system clipboard. Triggered by `Action::Copy`.
#[derive(EntityEvent, Debug)]
pub struct CopyToClipboardActionEvent {
    /// Target Terminal Activity entity.
    pub entity: Entity,
}

/// Request to paste the system clipboard into the focused terminal's
/// PTY. Triggered by `Action::Paste`.
#[derive(EntityEvent, Debug)]
pub struct PasteFromClipboardActionEvent {
    /// Target Terminal Activity entity.
    pub entity: Entity,
}

/// Bevy Plugin wiring the clipboard copy/paste action observers.
///
/// The `Clipboard` resource itself is inserted by `CopyModePlugin`; this
/// plugin only registers observers and must not re-insert it.
pub struct ClipboardActionPlugin;

impl Plugin for ClipboardActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_copy_to_clipboard)
            .add_observer(on_paste_from_clipboard);
    }
}

/// Observer for `CopyToClipboardActionEvent`. Writes the target terminal's
/// current selection to the system clipboard; no-ops on an empty
/// selection or a missing `TerminalHandle` (e.g. a Browser Activity).
fn on_copy_to_clipboard(
    ev: On<CopyToClipboardActionEvent>,
    mut clipboard: ResMut<Clipboard>,
    handles: Query<&TerminalHandle>,
) {
    let Ok(handle) = handles.get(ev.entity) else {
        return;
    };
    if let Some(text) = handle.selection_to_string()
        && !text.is_empty()
    {
        clipboard.write(text);
    }
}

/// Observer for `PasteFromClipboardActionEvent`. Reads the system clipboard
/// and writes it to the target terminal's PTY, wrapping in bracketed
/// paste markers when the terminal has bracketed paste enabled. No-ops
/// on an empty clipboard or a missing `TerminalHandle`.
fn on_paste_from_clipboard(
    ev: On<PasteFromClipboardActionEvent>,
    mut clipboard: ResMut<Clipboard>,
    mut handles: Query<(&mut TerminalHandle, &mut PtyHandle)>,
) {
    let Ok((mut handle, mut pty)) = handles.get_mut(ev.entity) else {
        return;
    };
    let Some(text) = clipboard.read().filter(|t| !t.is_empty()) else {
        return;
    };
    let bracketed = handle.bracketed_paste_enabled();
    let bytes = build_paste_bytes(&text, bracketed);
    if let Err(err) = handle.write(&mut pty, &bytes) {
        tracing::warn!(
            target: "ozmux_gui::clipboard",
            ?err,
            "paste PTY write failed",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_action_plugin_builds_without_panic() {
        use bevy::app::App;
        let mut app = App::new();
        app.insert_resource(Clipboard::new());
        app.add_plugins(super::ClipboardActionPlugin);
        app.update();
    }

    #[test]
    fn copy_observer_noops_on_entity_without_handle() {
        use bevy::app::App;
        use bevy::ecs::entity::Entity;
        let mut app = App::new();
        app.insert_resource(Clipboard::new());
        app.add_plugins(super::ClipboardActionPlugin);
        let e: Entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .trigger(super::CopyToClipboardActionEvent { entity: e });
        app.update();
    }

    #[test]
    fn paste_observer_noops_on_entity_without_handle() {
        use bevy::app::App;
        use bevy::ecs::entity::Entity;
        let mut app = App::new();
        app.insert_resource(Clipboard::new());
        app.add_plugins(super::ClipboardActionPlugin);
        let e: Entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .trigger(super::PasteFromClipboardActionEvent { entity: e });
        app.update();
    }

    #[test]
    fn read_returns_none_when_inner_is_unavailable() {
        // Force the unavailable-backend branch by constructing the
        // resource with `Clipboard(None)` directly. This mirrors what
        // `Clipboard::new` would do on a headless host where
        // `arboard::Clipboard::new()` fails.
        let mut cb = Clipboard(None);
        assert!(cb.read().is_none(), "headless backend must yield None");
    }

    #[test]
    fn build_paste_bytes_unbracketed_normalizes_crlf_to_cr() {
        let out = build_paste_bytes("foo\r\nbar", false);
        assert_eq!(out, b"foo\rbar");
    }

    #[test]
    fn build_paste_bytes_unbracketed_normalizes_lone_lf_to_cr() {
        let out = build_paste_bytes("foo\nbar", false);
        assert_eq!(out, b"foo\rbar");
    }

    #[test]
    fn build_paste_bytes_unbracketed_passes_existing_cr_through() {
        let out = build_paste_bytes("foo\rbar", false);
        assert_eq!(out, b"foo\rbar");
    }

    #[test]
    fn build_paste_bytes_bracketed_wraps_clean_body() {
        let out = build_paste_bytes("hello", true);
        assert_eq!(out, b"\x1b[200~hello\x1b[201~");
    }

    #[test]
    fn build_paste_bytes_bracketed_strips_embedded_7bit_end_marker() {
        let out = build_paste_bytes("foo\x1b[201~bar", true);
        assert_eq!(out, b"\x1b[200~foobar\x1b[201~");
    }

    #[test]
    fn build_paste_bytes_bracketed_strips_embedded_8bit_c1_end_marker() {
        let out = build_paste_bytes("foo\u{9b}201~bar", true);
        assert_eq!(out, b"\x1b[200~foobar\x1b[201~");
    }

    #[test]
    fn build_paste_bytes_bracketed_strips_embedded_start_markers() {
        let out_7 = build_paste_bytes("foo\x1b[200~bar", true);
        assert_eq!(out_7, b"\x1b[200~foobar\x1b[201~");
        let out_8 = build_paste_bytes("foo\u{9b}200~bar", true);
        assert_eq!(out_8, b"\x1b[200~foobar\x1b[201~");
    }

    #[test]
    fn build_paste_bytes_bracketed_loop_strips_nested_marker() {
        // "\x1b[\x1b[201~201~" — removing the inner marker re-exposes a
        // valid outer marker. A single replace pass would leave one;
        // the fixed-point loop must strip both.
        let out = build_paste_bytes("\x1b[\x1b[201~201~", true);
        assert_eq!(
            out, b"\x1b[200~\x1b[201~",
            "fixed-point loop must keep stripping until the body is stable",
        );
    }

    #[test]
    fn build_paste_bytes_empty_input_emits_well_formed_bytes() {
        assert_eq!(build_paste_bytes("", false), b"");
        assert_eq!(build_paste_bytes("", true), b"\x1b[200~\x1b[201~");
    }
}
