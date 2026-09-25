//! Caret paint policy: focus, hollow rendering, and the blink phase.

use crate::pane_style::PaneInactiveStyle;
use crate::schema::TerminalView;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use std::time::Duration;

mod blink;
mod paint;
mod style;

pub use blink::{blink_phase_on, next_blink_flip};
pub use paint::{CaretPaint, CaretPaintInput, CaretStroke, PackedCursorStyle};
pub use style::CaretStyle;

/// The real-time elapsed reading at the last keystroke, which the blink
/// phase counts from. A terminal that has seen no keystroke counts from
/// startup.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct LastKeyInstant(pub Duration);

/// The next blink-phase change of a painted caret.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NextCaretFlip(Option<Duration>);

impl NextCaretFlip {
    /// A reading whose next phase change comes at `at`, a `Time<Real>`
    /// elapsed reading; `None` means no painted caret blinks.
    pub fn new(at: Option<Duration>) -> Self {
        Self(at)
    }

    /// The `Time<Real>` elapsed reading at which the phase next changes, or
    /// `None` when no painted caret blinks.
    pub fn at(&self) -> Option<Duration> {
        self.0
    }
}

/// Registers the resources the cursor paint policy reads.
pub struct CursorPlugin;

impl Plugin for CursorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CaretStyle>()
            .init_resource::<LastKeyInstant>()
            .init_resource::<NextCaretFlip>()
            .add_systems(PostUpdate, publish_next_caret_flip);
    }
}

/// Publishes when the next painted caret changes its blink phase, writing
/// `NextCaretFlip` only when the reading changes.
fn publish_next_caret_flip(
    mut next: ResMut<NextCaretFlip>,
    terminals: Query<(&TerminalView, Has<PaneInactiveStyle>)>,
    window: Query<&Window, With<PrimaryWindow>>,
    style: Res<CaretStyle>,
    last_key: Res<LastKeyInstant>,
    time: Res<Time<Real>>,
) {
    let window_focused = window.single().is_ok_and(|window| window.focused);
    let blinking = terminals.iter().any(|(view, inactive)| {
        view.caret().is_some_and(|(_, cursor)| {
            CaretStroke::blinks(cursor, view.suppress_cursor, window_focused && !inactive)
        })
    });
    let flip = if blinking {
        next_blink_flip(
            time.elapsed().saturating_sub(last_key.0),
            style.blink_interval,
            style.blink_timeout,
        )
        .and_then(|offset| last_key.0.checked_add(offset))
    } else {
        None
    };
    next.set_if_neq(NextCaretFlip::new(flip));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use orzma_vt::prelude::{Cursor, CursorShape, GridColumn, GridLine, GridPoint};

    fn app(window_focused: bool) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .add_plugins(CursorPlugin);
        app.world_mut().spawn((
            Window {
                focused: window_focused,
                ..default()
            },
            PrimaryWindow,
        ));
        app
    }

    fn terminal(blinking: bool) -> TerminalView {
        TerminalView {
            cols: 80,
            rows: 24,
            cursor: Some(Cursor {
                point: GridPoint {
                    line: GridLine(5),
                    column: GridColumn(3),
                },
                shape: CursorShape::Block,
                blinking,
                visible: true,
            }),
            ..default()
        }
    }

    fn next_flip(app: &App) -> Option<Duration> {
        app.world().resource::<NextCaretFlip>().at()
    }

    /// Asserts that a focused, active, blinking caret publishes its next
    /// phase change as a `Time<Real>` elapsed reading.
    ///
    /// Case: the user has just started orzma and the prompt's caret blinks.
    #[test]
    fn a_focused_blinking_caret_publishes_its_next_phase_change() {
        let mut app = app(true);
        app.world_mut().spawn(terminal(true));
        app.update();
        assert_eq!(next_flip(&app), Some(Duration::from_millis(750)));
    }

    /// Asserts that no flip is published for an unfocused window or an
    /// inactive pane.
    ///
    /// Case: the user switches to another application, and separately
    /// looks at a split whose other pane holds the focus.
    #[test]
    fn an_unfocused_window_or_an_inactive_pane_publishes_no_flip() {
        let mut unfocused = app(false);
        unfocused.world_mut().spawn(terminal(true));
        unfocused.update();
        assert_eq!(next_flip(&unfocused), None);

        let mut split = app(true);
        split
            .world_mut()
            .spawn((terminal(true), PaneInactiveStyle::default()));
        split.update();
        assert_eq!(next_flip(&split), None);
    }

    /// Asserts that no flip is published for a steady caret or a caret the
    /// host suppresses.
    ///
    /// Case: vim switches to a steady block, and separately an IME
    /// composition opens over the prompt.
    #[test]
    fn a_steady_or_suppressed_caret_publishes_no_flip() {
        let mut steady = app(true);
        steady.world_mut().spawn(terminal(false));
        steady.update();
        assert_eq!(next_flip(&steady), None);

        let mut suppressed = app(true);
        suppressed.world_mut().spawn(TerminalView {
            suppress_cursor: true,
            ..terminal(true)
        });
        suppressed.update();
        assert_eq!(next_flip(&suppressed), None);
    }

    /// Asserts that the published flip clears once the blink times out.
    ///
    /// Case: the user leaves the keyboard alone for longer than the 5 s
    /// blink timeout.
    #[test]
    fn the_flip_clears_once_the_blink_times_out() {
        let mut app = app(true);
        app.world_mut().spawn(terminal(true));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(6)));
        app.update();
        app.update();
        assert_eq!(next_flip(&app), None);
    }

    /// Asserts that an unchanged reading leaves the resource unchanged.
    ///
    /// Case: two updates land inside the same lit blink phase.
    #[test]
    fn an_unchanged_reading_leaves_the_resource_unchanged() {
        let mut app = app(true);
        app.world_mut().spawn(terminal(true));
        app.update();
        let first = app.world().resource_ref::<NextCaretFlip>().last_changed();
        app.update();
        assert_eq!(next_flip(&app), Some(Duration::from_millis(750)));
        assert_eq!(
            app.world().resource_ref::<NextCaretFlip>().last_changed(),
            first
        );
    }
}
