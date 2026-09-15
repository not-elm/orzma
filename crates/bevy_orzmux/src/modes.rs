//! The modes a terminal's VT is in, kept in step with the backend's mode
//! reports.

use crate::signals::TtyModesSignal;
use bevy::prelude::*;
use orzma_vt::prelude::VtModes;

/// The modes a terminal's VT is in, as the backend last reported them;
/// [`VtModes::default`] until the first report arrives.
///
/// A report that repeats the modes already held leaves the component
/// untouched.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TtyModes(pub VtModes);

/// Keeps [`TtyModes`] in step with a terminal's mode reports.
pub(crate) struct TtyModesPlugin;

impl Plugin for TtyModesPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_modes_changed);
    }
}

fn on_modes_changed(event: On<TtyModesSignal>, mut modes: Query<&mut TtyModes>) {
    if let Ok(mut current) = modes.get_mut(event.terminal) {
        current.set_if_neq(TtyModes(event.modes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OrzmuxPane;
    use orzma_vt::prelude::MouseTracking;
    use orzmux::prelude::PaneId;

    #[derive(Resource, Default)]
    struct ChangedModes(usize);

    fn count_changed_modes(mut seen: ResMut<ChangedModes>, modes: Query<(), Changed<TtyModes>>) {
        seen.0 += modes.iter().count();
    }

    /// Builds an app with the modes observer and one entity holding
    /// default modes, with the spawn's own change notification already
    /// drained.
    fn app_with_modes() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(TtyModesPlugin)
            .init_resource::<ChangedModes>()
            .add_systems(Update, count_changed_modes);
        let terminal = app.world_mut().spawn(TtyModes::default()).id();
        app.update();
        app.world_mut().resource_mut::<ChangedModes>().0 = 0;
        (app, terminal)
    }

    fn tracking() -> VtModes {
        VtModes {
            mouse_tracking: MouseTracking::Drag,
            ..VtModes::default()
        }
    }

    /// Asserts that a modes signal writes the reported modes into the
    /// component.
    ///
    /// Case: nvim starts in a pane and turns on button-event tracking.
    #[test]
    fn a_modes_signal_writes_the_component() {
        let (mut app, terminal) = app_with_modes();
        app.world_mut().trigger(TtyModesSignal {
            terminal,
            modes: tracking(),
        });
        assert_eq!(
            app.world().get::<TtyModes>(terminal),
            Some(&TtyModes(tracking()))
        );
    }

    /// Asserts that a signal carrying the modes the component already
    /// holds leaves it unchanged rather than rewriting it.
    ///
    /// Case: a mode report arrives for a pane whose component already
    /// holds exactly those modes.
    #[test]
    fn a_repeated_modes_signal_does_not_mark_the_component_changed() {
        let (mut app, terminal) = app_with_modes();
        app.world_mut().trigger(TtyModesSignal {
            terminal,
            modes: tracking(),
        });
        app.update();
        assert_eq!(app.world().resource::<ChangedModes>().0, 1);

        app.world_mut().trigger(TtyModesSignal {
            terminal,
            modes: tracking(),
        });
        app.update();
        assert_eq!(app.world().resource::<ChangedModes>().0, 1);
    }

    /// Asserts that spawning a pane brings default modes with it.
    ///
    /// Case: the backend opens a pane and the user spins the wheel over
    /// it on the same frame, before any mode report has arrived.
    #[test]
    fn a_pane_requires_modes() {
        let mut app = App::new();
        let terminal = app.world_mut().spawn(OrzmuxPane(PaneId(1))).id();
        assert_eq!(
            app.world().get::<TtyModes>(terminal),
            Some(&TtyModes(VtModes::default()))
        );
    }
}
