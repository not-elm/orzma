//! Paste action pipeline: `on_paste` reads the system clipboard for a
//! `PasteAction` target and hands the text to the paste applier as
//! `PasteToTerminal`; also hosts `build_paste_bytes`, the shared paste-byte
//! construction.

use crate::{
    action::clipboard::paste::default_mode::PasteDefaultModePlugin,
    surface::OrzmaTerminal,
};
use bevy::{clipboard::ClipboardError, prelude::*};

mod default_mode;

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
        app.add_observer(on_paste)
            .add_plugins(PasteDefaultModePlugin);
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
fn build_paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
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

/// Carries clipboard text to paste into a specific terminal entity. Emitted
/// by `on_paste` once the clipboard has been read, so the paste applier never
/// touches the clipboard resource and stays testable by triggering this event
/// directly.
#[derive(EntityEvent, Debug, Clone)]
struct PasteToTerminal {
    /// The terminal entity to paste into.
    #[event_target]
    terminal: Entity,
    /// The non-empty clipboard text to paste.
    text: String,
}

/// The decision `on_paste` derives from a clipboard read poll. Keeping the
/// branch logic a pure function of the poll result lets every arm be
/// unit-tested without a real clipboard backend (bevy's `Clipboard` exposes no
/// in-memory seam).
enum PasteRead {
    /// Non-empty clipboard text ready to paste.
    Ready(String),
    /// Nothing to paste — empty clipboard or a fetch not yet resolved.
    Nothing,
    /// No content or no clipboard backend (headless); logged at debug.
    Unavailable,
    /// The clipboard read failed; logged at warn with the error.
    Failed(ClipboardError),
}

impl PasteRead {
    fn classify(read: Option<Result<String, ClipboardError>>) -> Self {
        match read {
            Some(Ok(text)) if !text.is_empty() => Self::Ready(text),
            Some(Ok(_)) | None => Self::Nothing,
            // NOTE: ClipboardNotSupported (headless / no backend) is grouped
            // with ContentNotAvailable into the debug-logged Unavailable
            // outcome on purpose — a later edit must not let it fall through to
            // Failed. Failed logs at warn, so a clipboard-less host would then
            // emit a warning on every paste keystroke, masking real warnings.
            Some(Err(
                ClipboardError::ContentNotAvailable | ClipboardError::ClipboardNotSupported,
            )) => Self::Unavailable,
            Some(Err(err)) => Self::Failed(err),
        }
    }
}

fn on_paste(
    ev: On<PasteAction>,
    mut commands: Commands,
    mut clipboard: ResMut<Clipboard>,
    targets: Query<(), With<OrzmaTerminal>>,
) {
    if targets.get(ev.entity).is_err() {
        return;
    }
    match PasteRead::classify(clipboard.fetch_text().poll_result()) {
        PasteRead::Ready(text) => {
            commands.trigger(PasteToTerminal {
                terminal: ev.entity,
                text,
            });
        }
        PasteRead::Nothing => {}
        PasteRead::Unavailable => {
            tracing::debug!(
                target: "orzma::clipboard",
                "paste clipboard read: no content or no backend (empty / non-text / headless)",
            );
        }
        PasteRead::Failed(err) => {
            tracing::warn!(
                target: "orzma::clipboard",
                error = ?err,
                "paste clipboard read failed",
            );
        }
    }
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
    fn classify_paste_read_ready_on_nonempty_text() {
        assert!(matches!(
            PasteRead::classify(Some(Ok("hi".to_string()))),
            PasteRead::Ready(text) if text == "hi"
        ));
    }

    #[test]
    fn classify_paste_read_nothing_on_empty_text() {
        assert!(matches!(
            PasteRead::classify(Some(Ok(String::new()))),
            PasteRead::Nothing
        ));
    }

    #[test]
    fn classify_paste_read_nothing_on_pending_fetch() {
        assert!(matches!(PasteRead::classify(None), PasteRead::Nothing));
    }

    #[test]
    fn classify_paste_read_unavailable_on_no_content_or_backend() {
        assert!(matches!(
            PasteRead::classify(Some(Err(ClipboardError::ContentNotAvailable))),
            PasteRead::Unavailable
        ));
        assert!(matches!(
            PasteRead::classify(Some(Err(ClipboardError::ClipboardNotSupported))),
            PasteRead::Unavailable
        ));
    }

    #[test]
    fn classify_paste_read_failed_on_other_error() {
        assert!(matches!(
            PasteRead::classify(Some(Err(ClipboardError::Unknown {
                description: "boom".to_string(),
            }))),
            PasteRead::Failed(_)
        ));
    }
}
