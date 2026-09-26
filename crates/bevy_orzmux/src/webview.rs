//! The webview host's events, re-addressed from panes to their entities and
//! triggered one by one.

use crate::registry::PaneRegistry;
use bevy::prelude::*;
use orzmux::prelude::{CommandSeq, PaneId, WebviewEvent};

/// One event of the webview host, with the pane of a mount resolved to the
/// pane's entity.
///
/// Events are triggered one at a time in the order the backend sent them,
/// so an observer sees every entity an earlier event's observers spawned or
/// despawned.
#[derive(Event, Debug, Clone, PartialEq)]
pub struct OrzmuxWebviewEvent {
    event: WebviewEvent<Entity>,
    seq: CommandSeq,
}

impl OrzmuxWebviewEvent {
    /// The host's `event`, stamped with `seq`.
    pub fn new(event: WebviewEvent<Entity>, seq: CommandSeq) -> Self {
        Self { event, seq }
    }

    /// The host's event.
    pub fn webview_event(&self) -> &WebviewEvent<Entity> {
        &self.event
    }

    /// The last GUI command the backend had processed when the event arose.
    pub fn seq(&self) -> CommandSeq {
        self.seq
    }
}

/// Triggers `event` with its pane resolved through `registry`; a mount for a
/// pane the registry does not know is logged and dropped.
pub(crate) fn trigger_webview_event(
    commands: &mut Commands,
    registry: &PaneRegistry,
    event: WebviewEvent<PaneId>,
    seq: CommandSeq,
) {
    match event.try_map_pane(|pane| registry.entity_of(pane)) {
        Some(event) => commands.trigger(OrzmuxWebviewEvent::new(event, seq)),
        None => tracing::debug!("webview mount for an unknown pane dropped"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drain::DrainPlugin;
    use crate::requests::test_support::{app_with_channels, spawn_pane};
    use crossbeam_channel::Sender;
    use orzma_vt::prelude::{InstanceId, PlacementSize};
    use orzma_webview_host::prelude::{HandleId, MountId, MountSpec};
    use orzmux::prelude::OrzmuxEvent;

    #[derive(Resource, Default)]
    struct Seen(Vec<(WebviewEvent<Entity>, CommandSeq)>);

    fn app() -> (App, Sender<OrzmuxEvent>) {
        let (mut app, events, _commands) = app_with_channels(DrainPlugin);
        app.init_resource::<Seen>().add_observer(
            |ev: On<OrzmuxWebviewEvent>, mut seen: ResMut<Seen>| {
                seen.0.push((ev.webview_event().clone(), ev.seq()));
            },
        );
        (app, events)
    }

    fn mounted(pane: PaneId) -> WebviewEvent<PaneId> {
        WebviewEvent::Mounted {
            pane,
            mount: MountId::new(1),
            instance: InstanceId(5),
            spec: MountSpec::new(
                HandleId::from("h"),
                "orzma://h/index.html",
                PlacementSize { rows: 4, cols: 8 },
            ),
        }
    }

    /// Asserts that webview events are triggered one per event, in the
    /// order the backend sent them, each with its own sequence.
    ///
    /// Case: a program unregisters a page, and the user's click on another
    /// page is answered in the same drain.
    #[test]
    fn webview_events_are_triggered_in_order_with_their_seq() {
        let (mut app, events) = app();
        events
            .send(OrzmuxEvent::Webview {
                event: WebviewEvent::AssetReleased {
                    handle: HandleId::from("h"),
                },
                seq: CommandSeq(3),
            })
            .unwrap();
        events
            .send(OrzmuxEvent::Webview {
                event: WebviewEvent::FocusChanged { focused: None },
                seq: CommandSeq(4),
            })
            .unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![
                (
                    WebviewEvent::AssetReleased {
                        handle: HandleId::from("h"),
                    },
                    CommandSeq(3),
                ),
                (WebviewEvent::FocusChanged { focused: None }, CommandSeq(4)),
            ]
        );
    }

    /// Asserts that a mount names the entity of its pane.
    ///
    /// Case: a program in the first pane mounts its page, and the webview
    /// layer has to parent the page under that pane's entity.
    #[test]
    fn a_mount_names_the_entity_of_its_pane() {
        let (mut app, events) = app();
        let entity = spawn_pane(&mut app, PaneId(7));
        events
            .send(OrzmuxEvent::Webview {
                event: mounted(PaneId(7)),
                seq: CommandSeq(0),
            })
            .unwrap();
        app.update();
        let seen = &app.world().resource::<Seen>().0;
        assert_eq!(seen.len(), 1);
        assert!(matches!(
            &seen[0].0,
            WebviewEvent::Mounted { pane, .. } if *pane == entity
        ));
    }

    /// Asserts that a mount for a pane the GUI does not know is dropped
    /// while the events after it are still triggered.
    ///
    /// Case: a pane's program mounts a page while the GUI holds no entity
    /// for that pane, because the pane's spawn answer never reached it.
    #[test]
    fn a_mount_for_an_unknown_pane_is_dropped() {
        let (mut app, events) = app();
        events
            .send(OrzmuxEvent::Webview {
                event: mounted(PaneId(99)),
                seq: CommandSeq(0),
            })
            .unwrap();
        events
            .send(OrzmuxEvent::Webview {
                event: WebviewEvent::Unmounted {
                    mounts: vec![MountId::new(1)],
                },
                seq: CommandSeq(0),
            })
            .unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![(
                WebviewEvent::Unmounted {
                    mounts: vec![MountId::new(1)],
                },
                CommandSeq(0),
            )]
        );
    }
}
