//! Clipboard Bevy Resource wrapping `arboard::Clipboard`, plus `build_paste_bytes`,
//! the pure helper that turns clipboard text into the PTY byte stream.

use bevy::prelude::*;

/// Resource wrapping a clipboard backend.
///
/// `arboard::Clipboard::new()` can fail when no display is available
/// (e.g. headless CI). In that case the backend is unavailable and every
/// `write` call becomes a no-op (logged at debug level once at init, then
/// silently dropped).
/// Copy-mode UI keeps working — the user can still see the selection —
/// but `y` does not modify the host clipboard. `Clipboard::in_memory`
/// swaps in a process-local backend for deterministic, headless-safe tests.
#[derive(Resource)]
pub(crate) struct Clipboard(ClipboardBackend);

/// The concrete clipboard backend behind a [`Clipboard`] resource.
enum ClipboardBackend {
    System(arboard::Clipboard),
    #[cfg(test)]
    Memory(Option<String>),
    Unavailable,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    /// Returns a clipboard backed by a process-local in-memory buffer.
    ///
    /// Reads observe exactly what was last written, with no dependency on a
    /// display server — used by tests to exercise copy/paste wiring on
    /// headless hosts where `arboard` is unavailable, without clobbering the
    /// developer's real clipboard.
    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self(ClipboardBackend::Memory(None))
    }

    /// Writes `text` to the clipboard. No-op when the backend is unavailable.
    /// Failures are logged at warn but never propagated — copy mode must not
    /// panic on a clipboard failure.
    pub(crate) fn write(&mut self, text: String) {
        match &mut self.0 {
            ClipboardBackend::System(cb) => {
                if let Err(e) = cb.set_text(text) {
                    tracing::warn!(
                        target: "orzma::clipboard",
                        error = ?e,
                        "clipboard write failed",
                    );
                }
            }
            #[cfg(test)]
            ClipboardBackend::Memory(slot) => *slot = Some(text),
            ClipboardBackend::Unavailable => {
                tracing::debug!(
                    target: "orzma::clipboard",
                    "clipboard write skipped: arboard unavailable",
                );
            }
        }
    }

    /// Reads text from the clipboard. Returns `None` when the backend is
    /// unavailable (headless) or when the clipboard does not currently hold
    /// UTF-8 text. Empty strings are passed through as `Some(String::new())`;
    /// the caller is responsible for treating empty as a no-op.
    ///
    /// arboard's behavior on an empty clipboard is platform-dependent —
    /// some backends return `Err(ContentNotAvailable)`, others return
    /// `Ok("")`. Both shapes are handled here (the `Err` path returns
    /// `None`, the `Ok("")` path returns `Some("")`); either way the
    /// caller's `text.is_empty()` check at the dispatcher swallows it
    /// without reaching the PTY.
    pub(crate) fn read(&mut self) -> Option<String> {
        match &mut self.0 {
            ClipboardBackend::System(cb) => match cb.get_text() {
                Ok(text) => Some(text),
                Err(arboard::Error::ContentNotAvailable) => {
                    tracing::debug!(
                        target: "orzma::clipboard",
                        "clipboard read: nothing available (empty / non-text)",
                    );
                    None
                }
                Err(err) => {
                    tracing::warn!(
                        target: "orzma::clipboard",
                        error = ?err,
                        "clipboard read failed",
                    );
                    None
                }
            },
            #[cfg(test)]
            ClipboardBackend::Memory(slot) => slot.clone(),
            ClipboardBackend::Unavailable => {
                tracing::debug!(
                    target: "orzma::clipboard",
                    "clipboard read skipped: arboard unavailable",
                );
                None
            }
        }
    }

    fn new() -> Self {
        match arboard::Clipboard::new() {
            Ok(cb) => Self(ClipboardBackend::System(cb)),
            Err(e) => {
                tracing::warn!(
                    target: "orzma::clipboard",
                    error = ?e,
                    "arboard init failed; clipboard writes will no-op",
                );
                Self(ClipboardBackend::Unavailable)
            }
        }
    }
}

/// Registers the shared `Clipboard` resource consumed by the action layer,
/// vi-mode UIs, and tmux paste.
pub(crate) struct ClipboardPlugin;

impl Plugin for ClipboardPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Clipboard>();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_returns_none_when_inner_is_unavailable() {
        // Mirrors what `Clipboard::new` produces on a headless host where
        // `arboard::Clipboard::new()` fails: the unavailable backend.
        let mut cb = Clipboard(ClipboardBackend::Unavailable);
        assert!(cb.read().is_none(), "headless backend must yield None");
    }

    #[test]
    fn in_memory_backend_round_trips_writes() {
        let mut cb = Clipboard::in_memory();
        assert!(cb.read().is_none(), "empty in-memory backend yields None");
        cb.write("hello world".to_string());
        assert_eq!(cb.read().as_deref(), Some("hello world"));
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
