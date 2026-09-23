//! Reports webview focus changes to the program that registered the
//! placement, as `focus_changed` pushes over its control connection.

use crate::control_plane::protocol::PushMsg;
use crate::control_plane::{ConnectionWriters, HandleId, OrzmaRegistry};
use crate::webview::mount::Webview;
use bevy::prelude::*;
use bevy_cef::prelude::FocusedWebview;
use orzma_vt::prelude::InstanceId;

/// Registers the system that pushes `focus_changed` to owning programs.
pub(crate) struct FocusPushPlugin;

impl Plugin for FocusPushPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            push_focus_changes.run_if(resource_exists_and_changed::<FocusedWebview>),
        );
    }
}

/// Where one placement's focus changes are reported: its entity and the
/// connection, handle and instance it was registered under.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FocusRoute {
    entity: Entity,
    connection_id: u64,
    handle: HandleId,
    instance: InstanceId,
}

impl FocusRoute {
    /// Resolves the route of `entity`, or `None` when it is not a mounted
    /// webview of a live registration.
    fn resolve(
        entity: Entity,
        webviews: &Query<&Webview>,
        registry: &OrzmaRegistry,
    ) -> Option<Self> {
        let webview = webviews.get(entity).ok()?;
        let view = registry.get(&webview.handle)?;
        Some(Self {
            entity,
            connection_id: view.connection_id,
            handle: webview.handle.clone(),
            instance: webview.instance,
        })
    }

    /// Queues a `focus_changed` push carrying `focused` to the owning
    /// connection.
    fn send(&self, writers: &ConnectionWriters, focused: bool) {
        let msg = PushMsg::FocusChanged {
            handle: self.handle.clone(),
            instance: self.instance.to_string(),
            focused,
        };
        match serde_json::to_string(&msg) {
            Ok(line) => {
                writers.send(self.connection_id, line);
            }
            Err(e) => tracing::warn!(error = %e, "focus_changed push failed to serialize"),
        }
    }
}

/// Pushes `focused: false` to the program that owned the previously focused
/// placement, then `focused: true` to the one that owns the newly focused
/// placement.
///
/// The previous route is kept from when it gained focus, so its `false`
/// reaches its program even after the entity despawned or the handle was
/// released.
fn push_focus_changes(
    mut last: Local<Option<FocusRoute>>,
    focused: Res<FocusedWebview>,
    webviews: Query<&Webview>,
    registry: Res<OrzmaRegistry>,
    writers: Res<ConnectionWriters>,
) {
    let current = focused.0;
    if last.as_ref().map(|route| route.entity) == current {
        return;
    }
    if let Some(previous) = last.take() {
        previous.send(&writers, false);
    }
    let next = current.and_then(|entity| FocusRoute::resolve(entity, &webviews, &registry));
    if let Some(route) = &next {
        route.send(&writers, true);
    }
    *last = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::{OrzmaSource, OrzmaView};
    use crossbeam_channel::{Receiver, unbounded};
    use serde_json::{Value, json};

    fn push_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(FocusPushPlugin)
            .init_resource::<OrzmaRegistry>()
            .init_resource::<FocusedWebview>()
            .init_resource::<ConnectionWriters>();
        app
    }

    fn connect(app: &App, connection_id: u64) -> Receiver<String> {
        let (tx, rx) = unbounded();
        app.world()
            .resource::<ConnectionWriters>()
            .insert(connection_id, tx);
        rx
    }

    /// Registers `handle` for `connection_id` with `source`, mints its first
    /// instance, and spawns the mounted webview entity.
    fn mount_view(
        app: &mut App,
        handle: &str,
        connection_id: u64,
        source: OrzmaSource,
    ) -> (Entity, InstanceId) {
        let handle = HandleId::from(handle);
        let instance = {
            let mut registry = app.world_mut().resource_mut::<OrzmaRegistry>();
            registry.insert(
                handle.clone(),
                OrzmaView {
                    source,
                    entry: "index.html".into(),
                    interactive: true,
                    click_focus: true,
                    owner_surface: Entity::PLACEHOLDER,
                    connection_id,
                    forward_keys: vec![],
                    preload: vec![],
                    instances: Vec::new(),
                },
            );
            registry.mint_instance(&handle).expect("the handle mints")
        };
        let entity = app
            .world_mut()
            .spawn(Webview {
                handle,
                instance,
                slot: 0,
                rows: 10,
                cols: 40,
            })
            .id();
        (entity, instance)
    }

    fn inline() -> OrzmaSource {
        OrzmaSource::Inline("<h1>x</h1>".into())
    }

    fn focus(app: &mut App, entity: Option<Entity>) {
        app.world_mut().resource_mut::<FocusedWebview>().0 = entity;
        app.update();
    }

    fn pushes(rx: &Receiver<String>) -> Vec<Value> {
        rx.try_iter()
            .map(|line| serde_json::from_str(&line).expect("a push is JSON"))
            .collect()
    }

    fn change(handle: &str, instance: InstanceId, focused: bool) -> Value {
        json!({"op": "focus_changed", "handle": handle, "instance": instance.to_string(), "focused": focused})
    }

    /// Asserts that focusing a placement pushes `focused: true` to the
    /// program that registered it.
    ///
    /// Case: the user clicks the page a markdown viewer mounted.
    #[test]
    fn focusing_a_placement_pushes_true_to_its_owner() {
        let mut app = push_app();
        let rx = connect(&app, 1);
        let (page, instance) = mount_view(&mut app, "h1", 1, inline());
        app.update();
        focus(&mut app, Some(page));
        assert_eq!(pushes(&rx), vec![change("h1", instance, true)]);
    }

    /// Asserts that moving focus pushes `false` for the old placement before
    /// `true` for the new one, to each owner.
    ///
    /// Case: the user clicks from one program's page to another's, and then
    /// between two placements of one program.
    #[test]
    fn moving_focus_pushes_false_then_true_to_each_owner() {
        let mut app = push_app();
        let rx1 = connect(&app, 1);
        let rx2 = connect(&app, 2);
        let rx3 = connect(&app, 3);
        let (a, a_id) = mount_view(&mut app, "ha", 1, inline());
        let (b, b_id) = mount_view(&mut app, "hb", 2, inline());
        let (c, c_id) = mount_view(&mut app, "hc", 3, inline());
        let (d, d_id) = mount_view(&mut app, "hd", 3, inline());
        app.update();
        focus(&mut app, Some(a));
        pushes(&rx1);
        focus(&mut app, Some(b));
        assert_eq!(pushes(&rx1), vec![change("ha", a_id, false)]);
        assert_eq!(pushes(&rx2), vec![change("hb", b_id, true)]);
        focus(&mut app, Some(c));
        pushes(&rx3);
        focus(&mut app, Some(d));
        assert_eq!(
            pushes(&rx3),
            vec![change("hc", c_id, false), change("hd", d_id, true)]
        );
    }

    /// Asserts that clearing focus pushes `focused: false`, even after the
    /// placement's entity is gone.
    ///
    /// Case: the user presses the release-focus shortcut, and later a page is
    /// unmounted while it holds focus.
    #[test]
    fn losing_focus_pushes_false_even_for_a_despawned_placement() {
        let mut app = push_app();
        let rx = connect(&app, 1);
        let (a, a_id) = mount_view(&mut app, "ha", 1, inline());
        let (b, b_id) = mount_view(&mut app, "hb", 1, inline());
        app.update();
        focus(&mut app, Some(a));
        pushes(&rx);
        focus(&mut app, None);
        assert_eq!(pushes(&rx), vec![change("ha", a_id, false)]);
        focus(&mut app, Some(b));
        pushes(&rx);
        app.world_mut().despawn(b);
        focus(&mut app, None);
        assert_eq!(pushes(&rx), vec![change("hb", b_id, false)]);
    }

    /// Asserts that a placement of an unregistered handle gets no `true`,
    /// while one unregistered while focused still gets its `false`.
    ///
    /// Case: a program closes its control connection while its page holds
    /// focus, and a stale page of a released handle is clicked.
    #[test]
    fn an_unregistered_handle_gets_no_true_but_keeps_its_false() {
        let mut app = push_app();
        let rx = connect(&app, 1);
        let (a, a_id) = mount_view(&mut app, "ha", 1, inline());
        let (b, _) = mount_view(&mut app, "hb", 1, inline());
        app.update();
        focus(&mut app, Some(a));
        pushes(&rx);
        app.world_mut()
            .resource_mut::<OrzmaRegistry>()
            .remove(&HandleId::from("ha"));
        app.world_mut()
            .resource_mut::<OrzmaRegistry>()
            .remove(&HandleId::from("hb"));
        focus(&mut app, Some(b));
        assert_eq!(pushes(&rx), vec![change("ha", a_id, false)]);
    }

    /// Asserts that writing the same focus again pushes nothing.
    ///
    /// Case: the user clicks the page that already holds focus.
    #[test]
    fn refocusing_the_same_placement_pushes_nothing() {
        let mut app = push_app();
        let rx = connect(&app, 1);
        let (a, _) = mount_view(&mut app, "ha", 1, inline());
        app.update();
        focus(&mut app, Some(a));
        pushes(&rx);
        focus(&mut app, Some(a));
        assert!(pushes(&rx).is_empty());
    }

    /// Asserts that a display-only `url` view, which has no back-channel,
    /// still gets focus pushes.
    ///
    /// Case: a program mounts a remote page without the bridge and the user
    /// clicks it.
    #[test]
    fn a_display_only_url_view_gets_pushes() {
        let mut app = push_app();
        let rx = connect(&app, 1);
        let source = OrzmaSource::Url {
            url: "https://example.com/".into(),
            bridge: false,
        };
        let (page, instance) = mount_view(&mut app, "hu", 1, source);
        app.update();
        focus(&mut app, Some(page));
        assert_eq!(pushes(&rx), vec![change("hu", instance, true)]);
    }

    /// Asserts that focus on an entity that is not a mounted webview pushes
    /// nothing, and that the next move to a real placement still pushes
    /// `true`.
    ///
    /// Case: focus follows a pane whose surface itself is a webview, and the
    /// user then clicks a page mounted in another pane.
    #[test]
    fn a_focus_on_a_non_webview_entity_pushes_nothing() {
        let mut app = push_app();
        let rx = connect(&app, 1);
        let surface = app.world_mut().spawn_empty().id();
        let (page, instance) = mount_view(&mut app, "ha", 1, inline());
        app.update();
        focus(&mut app, Some(surface));
        assert!(pushes(&rx).is_empty());
        focus(&mut app, Some(page));
        assert_eq!(pushes(&rx), vec![change("ha", instance, true)]);
        focus(&mut app, Some(surface));
        assert_eq!(pushes(&rx), vec![change("ha", instance, false)]);
    }
}
