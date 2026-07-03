//! Mouse-report write action: delivers mouse-protocol bytes to a terminal's
//! backend (PTY when attached, `TerminalForwardInput` when detached).

use crate::action::terminal::{TerminalBackendQuery, TerminalForwardInput, apply_to_terminal};
use bevy::prelude::*;

/// Writes mouse-protocol report bytes to `entity`'s backend (PTY when
/// attached, `TerminalForwardInput` when detached).
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

/// Applies a `TerminalMouseWrite`: PTY write when attached, otherwise a
/// `TerminalForwardInput` to the host-owned backend router.
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
                tracing::warn!(?e, "ozma mouse pty write failed");
            }
        },
        |commands, _handle, entity| {
            commands.trigger(TerminalForwardInput {
                entity,
                bytes: ev.bytes.clone(),
            });
            false
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::OzmaTerminal;
    use ozma_tty_engine::TerminalHandle;

    #[test]
    fn detached_write_event_forwards_bytes() {
        #[derive(Resource, Default)]
        struct CapturedForward(Vec<Vec<u8>>);

        let mut app = App::new();
        app.init_resource::<CapturedForward>()
            .add_observer(on_terminal_mouse_write)
            .add_observer(
                |ev: On<TerminalForwardInput>, mut cap: ResMut<CapturedForward>| {
                    cap.0.push(ev.bytes.clone());
                },
            );

        let handle = TerminalHandle::detached(10, 5);
        let entity = app.world_mut().spawn((OzmaTerminal, handle)).id();

        app.world_mut().trigger(TerminalMouseWrite {
            entity,
            bytes: b"\x1b[<0;1;1M".to_vec(),
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<CapturedForward>().0,
            vec![b"\x1b[<0;1;1M".to_vec()],
            "TerminalMouseWrite on a PTY-less OzmaTerminal must emit TerminalForwardInput"
        );
    }
}
