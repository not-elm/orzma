//! Clipboard write seam and paste-byte construction. `ClipboardWriteRequest`
//! carries copied text from the copy/yank observers to `apply_clipboard_write`,
//! the one system that writes Bevy's `Clipboard` resource; `build_paste_bytes`
//! turns clipboard text into the PTY byte stream. The clipboard resource itself
//! is `bevy::clipboard::Clipboard`, provided by `DefaultPlugins`.

use bevy::clipboard::{Clipboard, ClipboardError};
use bevy::prelude::*;

/// Requests that `text` be written to the system clipboard.
///
/// A global (non-entity) event: the clipboard is process-global, so copy/yank
/// observers `commands.trigger` this and `apply_clipboard_write` performs the
/// single `Clipboard::set_text`. Decoupling this way keeps the copy observers
/// testable by capturing the request instead of round-tripping a real clipboard.
#[derive(Event, Debug, Clone)]
pub(crate) struct ClipboardWriteRequest {
    /// The text to place on the system clipboard.
    pub text: String,
}

/// Registers the clipboard write-seam observer. Bevy's `Clipboard` resource
/// and `bevy_clipboard::ClipboardPlugin` come from `DefaultPlugins`; this
/// plugin only adds orzma's write path on top.
pub(crate) struct ClipboardPlugin;

impl Plugin for ClipboardPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_clipboard_write);
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

fn apply_clipboard_write(ev: On<ClipboardWriteRequest>, mut clipboard: ResMut<Clipboard>) {
    match clipboard.set_text(ev.text.as_str()) {
        Ok(()) => {}
        Err(ClipboardError::ClipboardNotSupported) => {
            tracing::debug!(
                target: "orzma::clipboard",
                "clipboard write skipped: no system clipboard backend (headless)",
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "orzma::clipboard",
                error = ?err,
                "clipboard write failed",
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Test-only sink recording every `ClipboardWriteRequest`'s text, so copy /
    /// yank observers can be verified without round-tripping a real OS clipboard
    /// (unavailable when headless, and clobbering the developer's clipboard).
    #[derive(Resource, Default)]
    pub(crate) struct CapturedClipboardWrites(pub(crate) Vec<String>);

    /// Registers `CapturedClipboardWrites` plus an observer that appends each
    /// triggered `ClipboardWriteRequest`'s text to it.
    pub(crate) fn capture_clipboard_writes(app: &mut App) {
        app.init_resource::<CapturedClipboardWrites>().add_observer(
            |ev: On<ClipboardWriteRequest>, mut captured: ResMut<CapturedClipboardWrites>| {
                captured.0.push(ev.text.clone());
            },
        );
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
}
