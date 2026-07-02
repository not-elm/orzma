//! Per-command PTY-level terminal action events: mode-neutral apply observers
//! that write to a terminal surface's handle, backend, or the clipboard. This
//! root aggregates their per-file plugins and hosts the shared attached /
//! detached apply helper.

mod forward_input;
mod mouse_write;
mod open_uri;
mod paste;
mod selection;
mod viewport_scroll;

use bevy::prelude::*;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};

pub(crate) use forward_input::TerminalForwardInput;
pub(crate) use mouse_write::TerminalMouseWrite;
pub(crate) use open_uri::TerminalOpenUri;
pub(crate) use paste::PasteAction;
pub(crate) use selection::{
    TerminalSelectionClear, TerminalSelectionCopy, TerminalSelectionStart, TerminalSelectionUpdate,
};
pub(crate) use viewport_scroll::TerminalViewportScroll;

/// Aggregates the per-command terminal action plugins.
pub(super) struct TerminalActionPlugin;

impl Plugin for TerminalActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            mouse_write::MouseWritePlugin,
            open_uri::OpenUriPlugin,
            paste::PastePlugin,
            selection::SelectionPlugin,
            viewport_scroll::ViewportScrollPlugin,
        ));
    }
}

/// Applies one handle-touching mouse op to `entity`, branching on whether
/// the terminal is PTY-attached (apply through the coalescer) or detached
/// (mutate the VT only, then `flush_emit`). `detached` returns whether a
/// frame flush is needed (the write op forwards instead and returns false).
fn apply_to_terminal(
    commands: &mut Commands,
    handle: &mut TerminalHandle,
    pty: Option<Mut<PtyHandle>>,
    coalescer: Option<Mut<Coalescer>>,
    entity: Entity,
    attached: impl FnOnce(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
    detached: impl FnOnce(&mut Commands, &mut TerminalHandle, Entity) -> bool,
) {
    if let (Some(mut pty), Some(mut coalescer)) = (pty, coalescer) {
        attached(handle, &mut pty, &mut coalescer);
    } else if detached(commands, handle, entity) {
        handle.flush_emit(commands, entity);
    }
}
