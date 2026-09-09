//! Viewport scroll action: scrolls a terminal surface's viewport into / out of
//! scrollback.

use bevy::prelude::*;
use bevy_orzmux::prelude::RequestTtyScroll;
use orzma_vt::prelude::Scroll;

/// Scrolls `entity`'s viewport by `lines`, in the same sign convention as
/// `Scroll::Delta`: positive moves deeper into scrollback (toward older
/// output), negative moves toward the live tail.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalViewportScroll {
    /// The terminal entity to scroll.
    #[event_target]
    pub entity: Entity,
    /// Lines to scroll; positive moves deeper into scrollback.
    pub lines: i32,
}

/// Registers the viewport-scroll apply observer.
pub(super) struct ViewportScrollPlugin;

impl Plugin for ViewportScrollPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_viewport_scroll);
    }
}

/// Applies a `TerminalViewportScroll` by requesting the same signed delta on
/// the underlying tty.
fn on_terminal_viewport_scroll(ev: On<TerminalViewportScroll>, mut commands: Commands) {
    commands.trigger(RequestTtyScroll {
        terminal: ev.entity,
        scroll: Scroll::Delta(ev.lines),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct SeenScrolls(Vec<(Entity, Scroll)>);

    /// Asserts that `TerminalViewportScroll` is forwarded as a
    /// `RequestTtyScroll::Delta` carrying the same signed line count,
    /// unchanged, to the same entity.
    ///
    /// Case: the mouse-wheel dispatcher fires one `TerminalViewportScroll`
    /// per accumulated notch while the user spins the wheel over a terminal
    /// outside any app mouse-tracking mode.
    #[test]
    fn viewport_scroll_forwards_the_signed_delta_unchanged() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SeenScrolls>()
            .add_observer(on_terminal_viewport_scroll)
            .add_observer(|ev: On<RequestTtyScroll>, mut seen: ResMut<SeenScrolls>| {
                seen.0.push((ev.terminal, ev.scroll));
            });
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut()
            .trigger(TerminalViewportScroll { entity, lines: -3 });
        app.update();

        assert_eq!(
            app.world().resource::<SeenScrolls>().0,
            vec![(entity, Scroll::Delta(-3))]
        );
    }

    /// Asserts that a scroll aimed at a nonexistent entity does not panic.
    ///
    /// Case: a wheel-notch request in flight while its target terminal is
    /// torn down between dispatch and apply.
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
