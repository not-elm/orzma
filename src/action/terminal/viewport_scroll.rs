//! Viewport scroll action: scrolls a terminal surface's viewport into / out of
//! scrollback.

use crate::action::terminal::apply_to_terminal;
use bevy::prelude::*;
use ozma_terminal::OzmaTerminal;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};

/// Scrolls `entity`'s viewport by `lines` (negative = up / into history).
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalViewportScroll {
    /// The terminal entity to scroll.
    #[event_target]
    pub entity: Entity,
    /// Lines to scroll; negative scrolls up into scrollback.
    pub lines: i32,
}

/// Registers the viewport-scroll apply observer.
pub(super) struct ViewportScrollPlugin;

impl Plugin for ViewportScrollPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_viewport_scroll);
    }
}

/// Applies a `TerminalViewportScroll`.
fn on_terminal_viewport_scroll(
    ev: On<TerminalViewportScroll>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
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
        |handle, _pty, coalescer| handle.scroll(coalescer, ev.lines),
        |_commands, handle, _entity| {
            handle.scroll_vt_only(ev.lines);
            true
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_scroll_event_on_missing_terminal_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_observer(on_terminal_viewport_scroll);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .trigger(TerminalViewportScroll { entity, lines: 3 });
        app.update();
    }
}
