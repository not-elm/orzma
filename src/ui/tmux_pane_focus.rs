//! Pane click-to-focus + dim: augments each tmux pane node with a `Button`
//! (click target) and a `FocusPolicy::Pass` dim overlay, sends `select-pane`
//! on click, and shows the overlay on every pane except the active one.

use crate::theme;
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use ozma_tty_engine::TerminalHandle;
use ozmux_tmux::TmuxPane;

/// Points a pane at its dim-overlay child entity (O(1) lookup in `sync_pane_dim`).
#[derive(Component)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "field read by sync_pane_dim, added in a later task"
    )
)]
pub(crate) struct PaneDim(pub(crate) Entity);

/// Registers pane click-to-focus and dim systems.
pub struct OzmuxTmuxPaneFocusPlugin;

impl Plugin for OzmuxTmuxPaneFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, augment_tmux_pane);
    }
}

/// Gives each rendered pane (one that has its `TerminalHandle` but no `Button`
/// yet) a `Button` click target and a hidden `FocusPolicy::Pass` dim overlay
/// child, recorded on the pane as `PaneDim`. The `Without<Button>` filter makes
/// this run exactly once per pane.
fn augment_tmux_pane(
    mut commands: Commands,
    panes: Query<Entity, (With<TmuxPane>, With<TerminalHandle>, Without<Button>)>,
) {
    for pane in panes.iter() {
        let overlay = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    top: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    ..default()
                },
                BackgroundColor(theme::PANE_DIM_OVERLAY),
                FocusPolicy::Pass,
                Visibility::Hidden,
                ChildOf(pane),
            ))
            .id();
        commands.entity(pane).insert((Button, PaneDim(overlay)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_tmux::TmuxPane;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tmux_control_parser::{CellDims, PaneId};

    fn dims() -> CellDims {
        CellDims {
            width: 10,
            height: 5,
            xoff: 0,
            yoff: 0,
        }
    }

    #[test]
    fn augment_adds_button_and_hidden_overlay() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, OzmuxTmuxPaneFocusPlugin));
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: dims(),
                },
                TerminalHandle::detached(10, 5, Arc::new(AtomicBool::new(false))),
            ))
            .id();
        app.update();

        assert!(
            app.world().get::<Button>(pane).is_some(),
            "pane gains a Button"
        );
        let pane_dim = app.world().get::<PaneDim>(pane).expect("PaneDim recorded");
        let overlay = pane_dim.0;
        assert_eq!(
            app.world().get::<Visibility>(overlay).copied(),
            Some(Visibility::Hidden),
            "overlay starts hidden",
        );
        assert_eq!(
            app.world().get::<FocusPolicy>(overlay).copied(),
            Some(FocusPolicy::Pass),
            "overlay passes clicks through to the pane",
        );

        app.update();
        let children = app
            .world()
            .get::<Children>(pane)
            .map(|c| c.len())
            .unwrap_or(0);
        assert_eq!(children, 1, "augment runs exactly once per pane");
    }
}
