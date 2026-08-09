//! `RequestTermPaste`: text the host UI asks a terminal entity to receive
//! as if typed.

use bevy::prelude::*;

use crate::OrzmaTermHandle;

/// Fired by the host UI to paste text into a specific terminal entity.
///
/// Carries the clipboard text verbatim.
/// Bracketed-paste framing, marker stripping, and line-ending normalization all depend on terminal modes the
/// host cannot see, so they belong to the apply observer — the host reads the clipboard and nothing more.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermPaste {
    #[event_target]
    pub terminal: Entity,
    /// The text to paste, exactly as read from the clipboard.
    pub text: String,
}

/// Registers the [`RequestTermPaste`] apply observer.
pub(super) struct PastePlugin;

impl Plugin for PastePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_paste);
    }
}

fn apply_paste(e: On<RequestTermPaste>, mut terms: Query<&mut OrzmaTermHandle>) {
    if let Ok(mut tty) = terms.get_mut(e.terminal)
        && let Err(err) = tty.write_paste(&e.text)
    {
        error!(%err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_term::test_support::CaptureSink;
    use orzma_vt::prelude::OrzmaVt;

    fn app_with_terminal() -> (App, Entity, CaptureSink) {
        let mut app = App::new();
        app.add_plugins(PastePlugin);
        let (handle, sink) = OrzmaTermHandle::detached(80, 24);
        let terminal = app.world_mut().spawn(handle).id();
        (app, terminal, sink)
    }

    fn trigger_paste(app: &mut App, terminal: Entity, text: &str) {
        app.world_mut().trigger(RequestTermPaste {
            terminal,
            text: text.to_owned(),
        });
    }

    /// Asserts that pasted text reaches the PTY writer through the
    /// mode-aware encoder: newline-normalized (`\n` / `\r\n` → `\r`) on
    /// a default, unbracketed VT.
    ///
    /// Case: the everyday ⌘V into a plain shell prompt — the clipboard
    /// carries editor/web line endings, but a shell expects one `\r`
    /// per line, exactly as the Enter key would produce. An observer
    /// that forwarded the clipboard verbatim would feed raw `\n` bytes
    /// to the PTY, breaking multi-line pastes; one that dropped the
    /// text entirely would write nothing. Both regressions land here.
    #[test]
    fn paste_writes_normalized_text_to_the_pty() {
        for (text, expected) in [("hello", b"hello".as_slice()), ("a\r\nb\nc", b"a\rb\rc")] {
            let (mut app, terminal, sink) = app_with_terminal();
            trigger_paste(&mut app, terminal, text);
            assert_eq!(
                sink.contents(),
                expected,
                "paste {text:?} must reach the PTY newline-normalized"
            );
        }
    }

    /// Asserts the observer consults the live VT for DECSET 2004 rather
    /// than encoding blind: the raw clipboard text reaches the
    /// mode-aware encoder unsanitized, and what lands on the PTY is the
    /// `ESC[200~ … ESC[201~` frame with embedded markers stripped.
    ///
    /// Case: pasting into an app that enabled bracketed paste (vim,
    /// fzf, modern shells). Only the VT knows the mode at paste time,
    /// so the host fires raw text and the apply layer must encode
    /// against the live mode — a layer that pre-sanitized or encoded
    /// blind would desync from the app. The second input is the paste-
    /// injection attack (markers smuggled inside the clipboard to break
    /// out of the frame and execute `rm -rf /`); it must arrive framed
    /// with the markers gone. Encoding branch coverage itself lives
    /// with `PtyInput::encode_paste`'s tests in `orzma_term`.
    #[test]
    fn paste_honours_live_bracketed_paste_mode() {
        for (text, expected) in [
            ("hi", b"\x1b[200~hi\x1b[201~".as_slice()),
            (
                "foo\x1b[201~rm -rf /\x1b[200~bar",
                b"\x1b[200~foorm -rf /bar\x1b[201~",
            ),
        ] {
            let mut app = App::new();
            app.add_plugins(PastePlugin);
            let (mut handle, sink) = OrzmaTermHandle::detached(80, 24);
            handle.vt_mut().interpret(b"\x1b[?2004h");
            let terminal = app.world_mut().spawn(handle).id();
            trigger_paste(&mut app, terminal, text);
            assert_eq!(
                sink.contents(),
                expected,
                "paste {text:?} must reach the PTY bracketed-framed"
            );
        }
    }

    /// Asserts that `#[event_target]` routing writes to the addressed
    /// terminal's PTY and to no other.
    ///
    /// Case: several terminal entities coexist (planned split panes)
    /// and the user pastes into the focused pane. A routing slip —
    /// the attribute moved to another field, or the observer querying
    /// the wrong entity — would inject clipboard bytes into a
    /// different pane's shell, and no compile error would catch it
    /// since every candidate field is an `Entity`.
    #[test]
    fn paste_targets_only_the_addressed_terminal() {
        let (mut app, target, target_sink) = app_with_terminal();
        let (other_handle, other_sink) = OrzmaTermHandle::detached(80, 24);
        app.world_mut().spawn(other_handle);
        trigger_paste(&mut app, target, "hello");
        assert_eq!(target_sink.contents(), b"hello");
        assert_eq!(other_sink.contents(), b"");
    }

    /// Asserts that a paste aimed at an entity with no terminal handle
    /// neither panics nor leaks into another terminal's PTY.
    ///
    /// Case: a request in flight while its target is torn down — the
    /// pane closed (handle removed) between the host reading the
    /// clipboard and the observer running. The `if let Ok` guard must
    /// swallow the miss; a panic here would take down the whole app
    /// for a lost race.
    #[test]
    fn paste_to_an_entity_without_a_handle_writes_nothing() {
        let (mut app, _terminal, sink) = app_with_terminal();
        let bare = app.world_mut().spawn_empty().id();
        trigger_paste(&mut app, bare, "hello");
        assert_eq!(sink.contents(), b"");
    }

    /// Asserts that an empty clipboard writes nothing at all to the PTY.
    ///
    /// Case: ⌘V with nothing (or an empty string) on the clipboard. In
    /// bracketed-paste mode a naive encode would still emit the
    /// `ESC[200~ESC[201~` frame and wake the receiving app for a
    /// zero-byte paste, so `write_paste` short-circuits before the
    /// encoder; this pins that no-op as observed through the observer.
    #[test]
    fn empty_paste_writes_nothing() {
        let (mut app, terminal, sink) = app_with_terminal();
        trigger_paste(&mut app, terminal, "");
        assert_eq!(sink.contents(), b"");
    }
}
