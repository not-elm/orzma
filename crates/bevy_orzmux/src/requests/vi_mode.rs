//! The vi-mode switch the host UI asks a terminal entity to perform.

use bevy::prelude::*;
pub use orzma_vt::prelude::ViModeSwitch;

/// Fired by the host UI to enter or leave vi mode on a specific terminal
/// entity. The backend has no vi mode, so applying it does nothing.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyViMode {
    #[event_target]
    pub terminal: Entity,
    /// Which direction to switch.
    pub switch: ViModeSwitch,
}

pub(super) struct ViModePlugin;

impl Plugin for ViModePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_vi_mode);
    }
}

fn apply_vi_mode(_e: On<RequestTtyViMode>) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `(target, switch)` an observer saw, in fire order.
    #[derive(Resource, Default)]
    struct Seen(Vec<(Entity, ViModeSwitch)>);

    /// Observer that appends what it received to [`Seen`].
    fn record(ev: On<RequestTtyViMode>, mut seen: ResMut<Seen>) {
        seen.0.push((ev.event_target(), ev.switch));
    }

    /// Asserts that both switch directions reach an observer with their target
    /// intact, in the order they were fired.
    ///
    /// Case: the enter/exit round trip a user performs constantly
    /// (`Ctrl-Shift-Space` in, `Esc` out).
    #[test]
    fn trigger_delivers_both_switch_directions_in_order() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();

        for switch in [ViModeSwitch::Enter, ViModeSwitch::Exit] {
            app.world_mut()
                .trigger(RequestTtyViMode { terminal, switch });
        }

        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![
                (terminal, ViModeSwitch::Enter),
                (terminal, ViModeSwitch::Exit)
            ]
        );
    }

    /// Asserts that a repeated switch is delivered every time rather than
    /// deduplicated.
    ///
    /// Case: `Enter` fired while already in vi mode.
    #[test]
    fn a_repeated_switch_is_not_deduplicated() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();

        for _ in 0..2 {
            app.world_mut().trigger(RequestTtyViMode {
                terminal,
                switch: ViModeSwitch::Enter,
            });
        }

        assert_eq!(app.world().resource::<Seen>().0.len(), 2);
    }
}
