//! `SelectWindowRequest` — switches to the target tmux window. The window
//! entity is resolved from the shortcut's index by the dispatcher, so this
//! observer only needs the projected `TmuxWindow` id.

use bevy::prelude::*;
use orzma_tmux::{SelectWindow, TmuxClient, TmuxWindow};

/// Switches to the tmux window owning `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct SelectWindowRequest {
    /// The window entity to select.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `SelectWindowRequest` apply observer.
pub(super) struct SelectWindowPlugin;

impl Plugin for SelectWindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_select_window);
    }
}

fn on_select_window(
    ev: On<SelectWindowRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    windows: Query<&TmuxWindow>,
) {
    let Ok(window) = windows.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if let Err(e) = client.send(SelectWindow { id: window.id }) {
        tracing::warn!(?e, "select-window send failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::WindowId;

    #[test]
    fn select_window_targets_window_id() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_select_window);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let window = app
            .world_mut()
            .spawn(TmuxWindow {
                id: WindowId(6),
                index: 2,
                name: "editor".into(),
            })
            .id();
        app.world_mut()
            .trigger(SelectWindowRequest { entity: window });
        app.update();
        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert!(out.contains("select-window -t @6"), "got {out:?}");
    }
}
