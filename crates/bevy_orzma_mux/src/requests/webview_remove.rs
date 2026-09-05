//! `RequestTtyWebviewRemove`: the placements the control plane asks a
//! terminal entity to drop when a registration is released.

use crate::OrzmaTtyHandle;
use bevy::prelude::*;
use orzma_vt::prelude::InstanceId;

/// Fired by the control plane to drop placements a terminal still holds
/// for registrations that are gone.
///
/// The VT cannot know that a registration was released — that fact lives
/// on the control socket — so without this the placements keep a cap slot
/// until their anchor scrolls out of history.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyWebviewRemove {
    #[event_target]
    pub terminal: Entity,
    /// The instances to drop; ids the terminal does not hold are ignored.
    pub instances: Vec<InstanceId>,
}

pub(super) struct WebviewRemovePlugin;

impl Plugin for WebviewRemovePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_webview_remove);
    }
}

fn apply_webview_remove(e: On<RequestTtyWebviewRemove>, mut terms: Query<&mut OrzmaTtyHandle>) {
    if let Ok(mut tty) = terms.get_mut(e.terminal) {
        tty.remove_placements(&e.instances);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the request drops the named placement from the
    /// terminal it targets and leaves an unnamed one standing.
    ///
    /// Case: one of two registrations on a pane is unregistered while
    /// both of its views are mounted.
    #[test]
    fn a_remove_request_drops_only_the_instances_it_names() {
        let a: InstanceId = "3f5a9c02d1e84b7690ab3cde12f45678"
            .parse()
            .expect("valid id");
        let b: InstanceId = "81b4e77c05a3492fd6180e29ba735fc1"
            .parse()
            .expect("valid id");
        let mut app = App::new();
        app.add_plugins(WebviewRemovePlugin);
        let (mut handle, _master) = OrzmaTtyHandle::detached(80, 24);
        handle.feed_bytes(format!("\x1b_Omount;n={a},r=4,c=8\x1b\\").as_bytes());
        handle.feed_bytes(format!("\x1b_Omount;n={b},r=4,c=8\x1b\\").as_bytes());
        let terminal = app.world_mut().spawn(handle).id();

        app.world_mut().trigger(RequestTtyWebviewRemove {
            terminal,
            instances: vec![a],
        });
        app.world_mut().flush();

        let mut handle = app
            .world_mut()
            .get_mut::<OrzmaTtyHandle>(terminal)
            .expect("the terminal keeps its handle");
        let frame = handle.pump().frame.expect("the removal changed the list");
        let ids: Vec<InstanceId> = frame
            .placements
            .expect("the placement list changed")
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec![b]);
    }
}
