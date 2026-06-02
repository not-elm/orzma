//! Split-pane shortcut action: splits the active pane along an orientation
//! when a `SplitPaneActionEvent` is triggered.
use bevy::prelude::*;
use ozmux_multiplexer::{MultiplexerCommands, Side, SplitOrientation};

/// Registers the `apply_split` observer for `SplitPaneActionEvent`.
pub struct SplitPaneActionPlugin;

impl Plugin for SplitPaneActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_split);
    }
}

/// Request to split the active pane along `orientation`. Triggered by
/// `ShortcutAction::SplitPane`.
#[derive(EntityEvent, Debug)]
pub struct SplitPaneActionEvent {
    #[event_target]
    pub session: Entity,
    pub orientation: SplitOrientation,
}

fn apply_split(trigger: On<SplitPaneActionEvent>, mut mux: MultiplexerCommands) {
    let SplitPaneActionEvent {
        session,
        orientation,
    } = trigger.event();
    let Some(active_pane) = mux.sessions_active_pane(*session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "Split: session vanished");
        return;
    };
    if let Err(e) = mux.split_pane(active_pane, Side::After, *orientation) {
        tracing::warn!(target: "ozmux_gui::commands", ?e, "split_pane failed");
    }
}
