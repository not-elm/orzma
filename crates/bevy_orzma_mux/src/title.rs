//! The `TtyTitle` component: the window title a terminal's application
//! last set, kept in step with the VT's title signals.

use crate::signals::{TtyTitleChangedSignal, TtyTitleResetSignal};
use bevy::prelude::*;

/// The window title a terminal's application last set through OSC 0 /
/// OSC 2; `None` until it sets one, and again after the VT reports the
/// title's return to the host's default.
///
/// The string arrives already sanitized by the VT's OSC parser, so
/// hosts can show it as is. A signal that repeats the state already
/// held leaves the component untouched, so a host gated on
/// `Changed<TtyTitle>` re-runs only when the title really moves.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub struct TtyTitle(pub Option<String>);

/// Registers the observers that write [`TtyTitle`] from the title
/// signals.
pub(crate) struct TtyTitlePlugin;

impl Plugin for TtyTitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_title_changed)
            .add_observer(on_title_reset);
    }
}

fn on_title_changed(event: On<TtyTitleChangedSignal>, mut titles: Query<&mut TtyTitle>) {
    if let Ok(mut title) = titles.get_mut(event.terminal)
        && title.0.as_deref() != Some(event.title.as_str())
    {
        title.0 = Some(event.title.clone());
    }
}

fn on_title_reset(event: On<TtyTitleResetSignal>, mut titles: Query<&mut TtyTitle>) {
    if let Ok(mut title) = titles.get_mut(event.terminal) {
        title.set_if_neq(TtyTitle(None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MuxPane;
    use orzma_mux::prelude::PaneId;

    #[derive(Resource, Default)]
    struct ChangedTitles(usize);

    fn count_changed_titles(mut seen: ResMut<ChangedTitles>, titles: Query<(), Changed<TtyTitle>>) {
        seen.0 += titles.iter().count();
    }

    /// Builds an app with the title observers and one titled entity,
    /// with the spawn's own change notification already drained.
    fn app_with_title() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(TtyTitlePlugin)
            .init_resource::<ChangedTitles>()
            .add_systems(Update, count_changed_titles);
        let terminal = app.world_mut().spawn(TtyTitle::default()).id();
        app.update();
        app.world_mut().resource_mut::<ChangedTitles>().0 = 0;
        (app, terminal)
    }

    /// Asserts that a title signal sets the component and a reset
    /// signal clears it.
    ///
    /// Case: vim sets the window title on start and, on exit, pops the
    /// empty title it had saved on the title stack.
    #[test]
    fn the_title_signals_write_the_component() {
        let (mut app, terminal) = app_with_title();
        app.world_mut().trigger(TtyTitleChangedSignal {
            terminal,
            title: "vim".to_string(),
        });
        assert_eq!(
            app.world().get::<TtyTitle>(terminal),
            Some(&TtyTitle(Some("vim".to_string())))
        );
        app.world_mut().trigger(TtyTitleResetSignal { terminal });
        assert_eq!(app.world().get::<TtyTitle>(terminal), Some(&TtyTitle(None)));
    }

    /// Asserts that a signal carrying the state already shown — the
    /// same title, or a reset of an already-reset title — leaves the
    /// component unchanged rather than rewriting it.
    ///
    /// Case: a shell prompt re-sends the same title on every command,
    /// and a program later pops an empty saved title from the title
    /// stack twice.
    #[test]
    fn a_repeated_title_does_not_mark_the_component_changed() {
        let (mut app, terminal) = app_with_title();
        app.world_mut().trigger(TtyTitleChangedSignal {
            terminal,
            title: "sh".to_string(),
        });
        app.update();
        assert_eq!(app.world().resource::<ChangedTitles>().0, 1);

        app.world_mut().trigger(TtyTitleChangedSignal {
            terminal,
            title: "sh".to_string(),
        });
        app.update();
        assert_eq!(app.world().resource::<ChangedTitles>().0, 1);

        app.world_mut().trigger(TtyTitleResetSignal { terminal });
        app.update();
        assert_eq!(app.world().resource::<ChangedTitles>().0, 2);

        app.world_mut().trigger(TtyTitleResetSignal { terminal });
        app.update();
        assert_eq!(app.world().resource::<ChangedTitles>().0, 2);
    }

    /// Asserts that a title signal addressed to one terminal leaves
    /// another terminal's title alone.
    ///
    /// Case: two terminals are open and only one of them runs a program
    /// that sets a title.
    #[test]
    fn a_title_signal_reaches_only_its_terminal() {
        let (mut app, terminal) = app_with_title();
        let other = app.world_mut().spawn(TtyTitle::default()).id();
        app.world_mut().trigger(TtyTitleChangedSignal {
            terminal,
            title: "vim".to_string(),
        });
        assert_eq!(app.world().get::<TtyTitle>(other), Some(&TtyTitle(None)));
    }

    /// Asserts that spawning a pane brings a default title with it.
    ///
    /// Case: the backend opens a pane and the host queries its title on
    /// the same frame, before any title signal has arrived.
    #[test]
    fn a_pane_requires_a_title() {
        let mut app = App::new();
        let terminal = app.world_mut().spawn(MuxPane(PaneId(1))).id();
        assert_eq!(app.world().get::<TtyTitle>(terminal), Some(&TtyTitle(None)));
    }
}
