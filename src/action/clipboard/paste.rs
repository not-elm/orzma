//! Paste action pipeline: `on_paste` reads the system clipboard for a
//! `PasteAction` target and hands the text to the mode-gated appliers as
//! `PasteToTerminal`; also hosts `build_paste_bytes`, the shared paste-byte
//! construction.

use crate::surface::OrzmaTerminal;
use bevy::{clipboard::ClipboardError, prelude::*};
use orzma_tmux::TmuxPane;

/// Pastes the system clipboard into the target terminal entity.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct PasteAction {
    /// The terminal entity to paste into.
    #[event_target]
    pub entity: Entity,
}

/// Registers the paste pipeline observers.
pub(super) struct ClipboardPasteActionPlugin;

impl Plugin for ClipboardPasteActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_paste);
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

/// Carries clipboard text to paste into a specific terminal or tmux pane
/// entity. Emitted by `on_paste` once the clipboard has been read, so the
/// mode appliers (`default_mode` / `tmux_mode`) never touch the clipboard
/// resource and stay testable by triggering this event directly.
#[derive(EntityEvent, Debug, Clone)]
struct PasteToTerminal {
    /// The terminal or tmux pane entity to paste into.
    #[event_target]
    terminal: Entity,
    /// The non-empty clipboard text to paste.
    text: String,
}

fn on_paste(
    ev: On<PasteAction>,
    mut commands: Commands,
    mut clipboard: ResMut<Clipboard>,
    targets: Query<(), Or<(With<OrzmaTerminal>, With<TmuxPane>)>>,
) {
    if targets.get(ev.entity).is_err() {
        return;
    }
    let text = match clipboard.fetch_text().poll_result() {
        Some(Ok(text)) => text,
        Some(Err(ClipboardError::ContentNotAvailable | ClipboardError::ClipboardNotSupported)) => {
            // NOTE: keep ClipboardNotSupported (headless / no backend) at debug,
            // matching on_copy and the pre-migration Clipboard::read; routing it
            // through the warn arm below spams a warning on every paste
            // keystroke on a clipboard-less host.
            tracing::debug!(
                target: "orzma::clipboard",
                "paste clipboard read: no content or no backend (empty / non-text / headless)",
            );
            return;
        }
        Some(Err(err)) => {
            tracing::warn!(
                target: "orzma::clipboard",
                error = ?err,
                "paste clipboard read failed",
            );
            return;
        }
        None => return,
    };
    if text.is_empty() {
        return;
    }
    commands.trigger(PasteToTerminal {
        terminal: ev.entity,
        text,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn on_paste_ignores_non_terminal_entity() {
        #[derive(Resource, Default)]
        struct Emitted(usize);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Clipboard>()
            .init_resource::<Emitted>()
            .add_observer(on_paste)
            .add_observer(|_ev: On<PasteToTerminal>, mut emitted: ResMut<Emitted>| {
                emitted.0 += 1;
            });
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(PasteAction { entity });
        app.update();
        assert_eq!(
            app.world().resource::<Emitted>().0,
            0,
            "a PasteAction on a non-terminal entity must not read the clipboard or emit PasteToTerminal"
        );
    }

    #[test]
    fn on_paste_does_not_panic_for_terminal_target() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_observer(on_paste)
            .init_resource::<Clipboard>();
        let entity = app.world_mut().spawn(OrzmaTerminal).id();
        // NOTE: assert only "does not panic" — do NOT assert on emitted
        // PasteToTerminal. With system_clipboard on, a dev box's arboard is
        // available and the reader emits PasteToTerminal from the real
        // (possibly non-empty) clipboard; asserting "no PasteToTerminal"
        // would be flaky. On headless CI the backend is unavailable and
        // nothing is emitted.
        app.world_mut().trigger(PasteAction { entity });
        app.update();
    }
}
