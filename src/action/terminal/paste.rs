//! Paste action: reads the system clipboard and writes it to the target
//! terminal entity's PTY as (optionally bracketed) paste bytes.

use crate::action::clipboard::build_paste_bytes;
use crate::surface::OrzmaTerminal;
use bevy::clipboard::{Clipboard, ClipboardError};
use bevy::prelude::*;
use orzma_tmux::TmuxPane;
use orzma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};

/// Pastes the system clipboard into the target terminal entity's PTY.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct PasteAction {
    /// The terminal entity to paste into.
    #[event_target]
    pub entity: Entity,
}

/// Carries clipboard text to paste into a specific terminal or tmux pane
/// entity. Emitted by `read_clipboard_for_paste` once the clipboard has been
/// read, so the appliers (`on_paste` / `on_paste_tmux`) never touch the
/// clipboard resource and stay testable by triggering this event directly.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct PasteText {
    /// The terminal or tmux pane entity to paste into.
    #[event_target]
    pub entity: Entity,
    /// The non-empty clipboard text to paste.
    pub text: String,
}

/// Registers the paste apply observer.
pub(super) struct PastePlugin;

impl Plugin for PastePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(read_clipboard_for_paste)
            .add_observer(on_paste);
    }
}

fn read_clipboard_for_paste(
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
            // matching apply_clipboard_write and the pre-migration Clipboard::read;
            // routing it through the warn arm below spams a warning on every paste
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
    commands.trigger(PasteText {
        entity: ev.entity,
        text,
    });
}

fn on_paste(
    ev: On<PasteText>,
    mut terminals: Query<
        (&mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
        (With<OrzmaTerminal>, Without<TmuxPane>),
    >,
) {
    let Ok((mut handle, mut pty, mut coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    if !handle.is_at_bottom() {
        handle.scroll_to_bottom(&mut coalescer);
    }
    let bracketed = handle.bracketed_paste_enabled();
    let bytes = build_paste_bytes(&ev.text, bracketed);
    if let Err(e) = handle.write(&mut pty, &bytes) {
        tracing::warn!(?e, entity = ?ev.entity, "orzma paste write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_paste_without_terminal_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(PastePlugin);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(PasteText {
            entity,
            text: "hello".to_string(),
        });
        app.update();
        // Reaching here proves on_paste handled the missing-terminal path
        // without panicking. Byte correctness is covered by the clipboard
        // `build_paste_bytes_*` tests.
    }

    #[test]
    fn on_paste_is_noop_for_tmux_pane() {
        use orzma_tmux::PaneId;
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(PastePlugin);

        let pane = app
            .world_mut()
            .spawn((
                OrzmaTerminal,
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 0,
                        height: 0,
                        xoff: 0,
                        yoff: 0,
                    },
                },
            ))
            .id();
        app.world_mut().trigger(PasteText {
            entity: pane,
            text: "hello".to_string(),
        });
        app.update();
        // Reaching here proves the PTY-write path was not taken: the tmux
        // pane entity has no PtyHandle/Coalescer, so on_paste's query cannot
        // match it regardless of the Without<TmuxPane> filter.
    }

    #[test]
    fn read_clipboard_for_paste_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(PastePlugin)
            .init_resource::<Clipboard>();
        let entity = app.world_mut().spawn(OrzmaTerminal).id();
        // NOTE: assert only "does not panic" — do NOT assert on emitted PasteText.
        // With system_clipboard on, a dev box's arboard is available and the reader
        // emits PasteText from the real (possibly non-empty) clipboard (on_paste then
        // drops it for lack of a PtyHandle); asserting "no PasteText" would be flaky.
        // On headless CI the backend is unavailable and nothing is emitted.
        app.world_mut().trigger(PasteAction { entity });
        app.update();
    }
}
