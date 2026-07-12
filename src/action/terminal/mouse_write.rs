//! Mouse-report write action: delivers mouse-protocol bytes to a terminal's PTY.

use crate::action::terminal::{TerminalBackendQuery, apply_to_terminal};
use bevy::prelude::*;

/// Writes mouse-protocol report bytes to `entity`'s backend (PTY when
/// attached; dropped when detached).
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalMouseWrite {
    /// The terminal entity whose backend receives `bytes`.
    #[event_target]
    pub entity: Entity,
    /// The report bytes to deliver.
    pub bytes: Vec<u8>,
}

/// Registers the mouse-write apply observer.
pub(super) struct MouseWritePlugin;

impl Plugin for MouseWritePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_mouse_write);
    }
}

/// Applies a `TerminalMouseWrite`: PTY write when attached;
/// a detached (PTY-less) terminal drops the bytes.
fn on_terminal_mouse_write(
    ev: On<TerminalMouseWrite>,
    mut commands: Commands,
    mut terminals: TerminalBackendQuery,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, pty, _coalescer| {
            if let Err(e) = handle.write(pty, &ev.bytes) {
                tracing::warn!(?e, "orzma mouse pty write failed");
            }
        },
        |_commands, _handle, _entity| false,
    );
}
