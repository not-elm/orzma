use bevy::prelude::*;
use ozmux_multiplexer::{MultiplexerCommands, Side, SplitOrientation};

pub struct SplitPaneActionPlugin;

impl Plugin for SplitPaneActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_split);
    }
}

#[derive(EntityEvent, Debug)]
pub struct SplitPaneEvent {
    #[event_target]
    pub session: Entity,
    pub orientation: SplitOrientation,
}

fn apply_split(trigger: On<SplitPaneEvent>, mut mux: MultiplexerCommands) {
    let SplitPaneEvent {
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
