//! Empties the backend's event channel every frame and turns each event
//! into entity state or an `EntityEvent` the host observes.

use crate::layout::CurrentLayout;
use crate::registry::PaneRegistry;
use crate::signals::{
    TtyChildExitSignal, TtyFrameSignal, TtySelectionTextSignal, TtyTitleSignal, trigger_vt_signal,
};
use crate::tab::{CurrentTabs, PendingTabMove};
use crate::webview::trigger_webview_event;
use crate::{OrzmuxConnection, OrzmuxPane, OrzmuxSystems};
use bevy::prelude::*;
use orzma_vt::prelude::Frame;
use orzmux::prelude::{CloseReason, OrzmuxEvent, PaneId};

/// The session is over: the last tab closed, or the backend is
/// gone.
#[derive(Event, Debug, Clone, Copy)]
pub struct OrzmuxSessionEnded;

/// A `NewPane` the backend refused; the host unbinds and despawns.
#[derive(EntityEvent, Debug, Clone)]
pub struct OrzmuxPaneSpawnFailed {
    #[event_target]
    pub entity: Entity,
    pub error: String,
}

/// Turns the backend's queued events into entity state and signals.
pub(crate) struct DrainPlugin;

impl Plugin for DrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PaneRegistry>()
            .init_resource::<CurrentLayout>()
            .init_resource::<CurrentTabs>()
            .init_resource::<PendingTabMove>()
            .add_systems(
                Update,
                drain_orzmux_events
                    .run_if(resource_exists::<OrzmuxConnection>)
                    .in_set(OrzmuxSystems::Drain),
            );
    }
}

/// Drains every queued event in order, then ends the session and removes
/// `OrzmuxConnection` once the backend is gone.
///
/// A disconnect ends the session exactly once: removing the connection
/// leaves nothing for a later call to drain.
fn drain_orzmux_events(
    mut commands: Commands,
    mut registry: ResMut<PaneRegistry>,
    mut current: ResMut<CurrentLayout>,
    mut tabs: ResMut<CurrentTabs>,
    mut pending_move: ResMut<PendingTabMove>,
    connection: Res<OrzmuxConnection>,
) {
    for event in connection.0.try_iter() {
        apply_event(
            &mut commands,
            &mut registry,
            &mut current,
            &mut tabs,
            &mut pending_move,
            event,
        );
    }
    if connection.0.is_disconnected() {
        commands.trigger(OrzmuxSessionEnded);
        commands.remove_resource::<OrzmuxConnection>();
    }
}

/// Applies one event. `current` and `tabs` are marked changed only
/// when their content differs.
fn apply_event(
    commands: &mut Commands,
    registry: &mut PaneRegistry,
    current: &mut ResMut<CurrentLayout>,
    tabs: &mut ResMut<CurrentTabs>,
    pending_move: &mut ResMut<PendingTabMove>,
    event: OrzmuxEvent,
) {
    match event {
        OrzmuxEvent::PaneOpened { pane, request } => {
            if let Some(entity) = registry.pending_spawns.remove(&request) {
                registry.panes.insert(pane, entity);
                commands.entity(entity).insert(OrzmuxPane(pane));
            }
        }
        OrzmuxEvent::SpawnFailed { request, error } => {
            if let Some(entity) = registry.pending_spawns.remove(&request) {
                commands.trigger(OrzmuxPaneSpawnFailed { entity, error });
            }
        }
        OrzmuxEvent::Layout { layout, frames } => {
            current.set_if_neq(CurrentLayout(layout));
            for (pane, frame) in frames {
                trigger_frame(commands, registry, pane, frame);
            }
        }
        OrzmuxEvent::Frame { pane, frame } => trigger_frame(commands, registry, pane, frame),
        OrzmuxEvent::Signal { pane, signal } => match registry.entity_of(pane) {
            Some(terminal) => trigger_vt_signal(commands, terminal, signal),
            None => tracing::debug!(?pane, "signal for an unknown pane dropped"),
        },
        OrzmuxEvent::PaneTitle { pane, title } => match registry.entity_of(pane) {
            Some(terminal) => commands.trigger(TtyTitleSignal { terminal, title }),
            None => tracing::debug!(?pane, "title for an unknown pane dropped"),
        },
        OrzmuxEvent::SelectionText { text } => commands.trigger(TtySelectionTextSignal { text }),
        OrzmuxEvent::SelectionCopied { text } => {
            commands.trigger(TtySelectionTextSignal { text: Some(text) });
        }
        OrzmuxEvent::PaneClosed { pane, reason } => {
            if let Some(entity) = registry.panes.remove(&pane) {
                let code = match reason {
                    CloseReason::ChildExit { code } => code,
                    CloseReason::Killed => None,
                };
                commands.trigger(TtyChildExitSignal { entity, code });
                commands.entity(entity).despawn();
            }
        }
        OrzmuxEvent::Webview { event, seq } => {
            trigger_webview_event(commands, registry, event, seq);
        }
        OrzmuxEvent::Tabs {
            seq,
            entries,
            active,
        } => {
            if !tabs.entries.is_empty() && entries.is_empty() {
                commands.trigger(OrzmuxSessionEnded);
            }
            tabs.set_if_neq(CurrentTabs { entries, active });
            if pending_move.0.is_some_and(|sent| seq >= sent) {
                pending_move.0 = None;
            }
        }
    }
}

fn trigger_frame(commands: &mut Commands, registry: &PaneRegistry, pane: PaneId, frame: Frame) {
    match registry.entity_of(pane) {
        Some(terminal) => commands.trigger(TtyFrameSignal { terminal, frame }),
        None => tracing::debug!(?pane, "frame for an unknown pane dropped"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::app_with_channels;
    use crate::signals::TtyFrameSignal;
    use crate::title::{TtyTitle, TtyTitlePlugin};
    use crossbeam_channel::Sender;
    use orzma_vt::prelude::{Cursor, DisplayOffset, GridSize};
    use orzmux::prelude::{CloseReason, CommandSeq, Layout, PaneRect, RequestId, TabEntry, TabId};

    #[derive(Resource, Default)]
    struct Seen {
        ended: usize,
        spawn_failed: Vec<(Entity, String)>,
        frames: Vec<Entity>,
        texts: Vec<Option<String>>,
        layout_changes: usize,
        tab_changes: usize,
    }

    fn app() -> (App, Sender<OrzmuxEvent>) {
        let (mut app, events, _commands) = app_with_channels(DrainPlugin);
        app.init_resource::<Seen>()
            .add_observer(|_: On<OrzmuxSessionEnded>, mut seen: ResMut<Seen>| seen.ended += 1)
            .add_observer(|ev: On<OrzmuxPaneSpawnFailed>, mut seen: ResMut<Seen>| {
                seen.spawn_failed.push((ev.entity, ev.error.clone()));
            })
            .add_observer(|ev: On<TtyFrameSignal>, mut seen: ResMut<Seen>| {
                seen.frames.push(ev.terminal)
            })
            .add_observer(|ev: On<TtySelectionTextSignal>, mut seen: ResMut<Seen>| {
                seen.texts.push(ev.text.clone());
            })
            .add_systems(
                Update,
                (
                    |current: Res<CurrentLayout>, mut seen: ResMut<Seen>| {
                        if current.is_changed() {
                            seen.layout_changes += 1;
                        }
                    },
                    |tabs: Res<CurrentTabs>, mut seen: ResMut<Seen>| {
                        if tabs.is_changed() {
                            seen.tab_changes += 1;
                        }
                    },
                )
                    .after(OrzmuxSystems::Drain),
            );
        (app, events)
    }

    fn frame(size: GridSize) -> Frame {
        Frame {
            size,
            rows: vec![],
            cursor: Cursor::default(),
            display_offset: DisplayOffset(0),
            history_len: 0,
            vi_cursor: None,
            selection: None,
            placements: None,
            palette: None,
            hyperlinks: vec![],
            wraps: None,
            continues_from_above: false,
        }
    }

    fn layout(seq: u64, panes: &[(PaneId, u16)]) -> Layout {
        Layout {
            seq: CommandSeq(seq),
            size: GridSize { cols: 80, rows: 24 },
            active: panes.first().map(|p| p.0),
            panes: panes
                .iter()
                .map(|(pane, x)| PaneRect {
                    pane: *pane,
                    x: *x,
                    y: 0,
                    cols: 10,
                    rows: 24,
                })
                .collect(),
            separators: vec![],
        }
    }

    /// Asserts that `PaneOpened` promotes the pending entity to a
    /// `OrzmuxPane` and that a `Frame` in the same drain reaches it.
    ///
    /// Case: the backend answers a spawn and immediately sends the
    /// pane's bootstrap frame in the same batch.
    #[test]
    fn pane_opened_promotes_the_pending_entity_and_routes_the_same_drains_frame() {
        let (mut app, events) = app();
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<PaneRegistry>()
            .pending_spawns
            .insert(RequestId(1), entity);
        events
            .send(OrzmuxEvent::PaneOpened {
                pane: PaneId(7),
                request: RequestId(1),
            })
            .unwrap();
        events
            .send(OrzmuxEvent::Frame {
                pane: PaneId(7),
                frame: frame(GridSize::new(80, 24).expect("a valid size")),
            })
            .unwrap();
        app.update();
        assert_eq!(
            app.world().get::<OrzmuxPane>(entity),
            Some(&OrzmuxPane(PaneId(7)))
        );
        assert_eq!(app.world().resource::<Seen>().frames, vec![entity]);
        assert!(
            app.world()
                .resource::<PaneRegistry>()
                .pending_spawns
                .is_empty()
        );
    }

    /// Asserts that `SpawnFailed` hands the pending entity to the host
    /// through `OrzmuxPaneSpawnFailed`.
    ///
    /// Case: the shell could not be spawned for a split.
    #[test]
    fn spawn_failed_reports_the_pending_entity() {
        let (mut app, events) = app();
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<PaneRegistry>()
            .pending_spawns
            .insert(RequestId(1), entity);
        events
            .send(OrzmuxEvent::SpawnFailed {
                request: RequestId(1),
                error: "no space".into(),
            })
            .unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<Seen>().spawn_failed,
            vec![(entity, "no space".to_string())]
        );
    }

    fn tabs(seq: u64, ids: &[u32]) -> OrzmuxEvent {
        OrzmuxEvent::Tabs {
            seq: CommandSeq(seq),
            entries: ids
                .iter()
                .map(|id| TabEntry {
                    id: TabId(*id),
                    name: None,
                    active_pane: PaneId(*id),
                })
                .collect(),
            active: ids.first().map(|id| TabId(*id)),
        }
    }

    /// Asserts that the session ends only when a non-empty tab list
    /// becomes empty, not when a `Layout` has no panes.
    ///
    /// Case: the displayed tab's last shell exits while another
    /// tab remains, and later the last tab's shell exits.
    #[test]
    fn the_session_ends_only_when_the_tab_list_empties() {
        let (mut app, events) = app();
        events.send(tabs(1, &[1, 2])).unwrap();
        events
            .send(OrzmuxEvent::Layout {
                layout: layout(1, &[(PaneId(1), 0)]),
                frames: vec![],
            })
            .unwrap();
        app.update();
        events
            .send(OrzmuxEvent::Layout {
                layout: layout(2, &[]),
                frames: vec![],
            })
            .unwrap();
        app.update();
        assert_eq!(app.world().resource::<Seen>().ended, 0);
        events.send(tabs(3, &[])).unwrap();
        app.update();
        assert_eq!(app.world().resource::<Seen>().ended, 1);
    }

    /// Asserts that a `Tabs` whose sequence reaches the pending
    /// move clears it, and an older one does not.
    ///
    /// Case: the user drops a tab while a rename of another tab is still
    /// being answered.
    #[test]
    fn a_tabs_at_or_after_the_move_clears_the_pending_move() {
        let (mut app, events) = app();
        app.world_mut().resource_mut::<PendingTabMove>().0 = Some(CommandSeq(5));
        events.send(tabs(4, &[1])).unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<PendingTabMove>().0,
            Some(CommandSeq(5))
        );
        events.send(tabs(5, &[1])).unwrap();
        app.update();
        assert_eq!(app.world().resource::<PendingTabMove>().0, None);
    }

    /// Asserts that `CurrentTabs` is marked changed only when its
    /// content actually differs between two `Tabs` events.
    ///
    /// Case: the backend answers a no-op tab drop with the same list, and
    /// the tab bar must not rebuild.
    #[test]
    fn current_tabs_changes_only_when_its_content_does() {
        let (mut app, events) = app();
        app.update();
        app.world_mut().resource_mut::<Seen>().tab_changes = 0;

        events.send(tabs(1, &[1, 2])).unwrap();
        app.update();
        assert_eq!(app.world().resource::<Seen>().tab_changes, 1);

        events.send(tabs(2, &[1, 2])).unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<Seen>().tab_changes,
            1,
            "an answer with the same entries and active is not marked changed"
        );

        events.send(tabs(3, &[1])).unwrap();
        app.update();
        assert_eq!(app.world().resource::<Seen>().tab_changes, 2);
    }

    /// Asserts that `PaneClosed` despawns the entity with its children
    /// and forgets the pane.
    ///
    /// Case: a pane with an inline webview child is killed.
    #[test]
    fn pane_closed_despawns_the_entity_recursively() {
        let (mut app, events) = app();
        let entity = app.world_mut().spawn(OrzmuxPane(PaneId(3))).id();
        let child = app.world_mut().spawn(ChildOf(entity)).id();
        app.world_mut()
            .resource_mut::<PaneRegistry>()
            .panes
            .insert(PaneId(3), entity);
        events
            .send(OrzmuxEvent::PaneClosed {
                pane: PaneId(3),
                reason: CloseReason::Killed,
            })
            .unwrap();
        app.update();
        assert!(app.world().get_entity(entity).is_err());
        assert!(app.world().get_entity(child).is_err());
        assert!(app.world().resource::<PaneRegistry>().panes.is_empty());
    }

    /// Asserts that a `PaneTitle` in the same drain as its pane's
    /// `PaneOpened` reaches the new pane's `TtyTitle`, and that a title for
    /// a pane with no entity is dropped.
    ///
    /// Case: the shell's startup file sets the title right as the backend
    /// answers the spawn, while a title races the close of another pane.
    #[test]
    fn a_pane_title_in_the_opening_drain_reaches_the_new_pane() {
        let (mut app, events) = app();
        app.add_plugins(TtyTitlePlugin);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<PaneRegistry>()
            .pending_spawns
            .insert(RequestId(1), entity);
        events
            .send(OrzmuxEvent::PaneOpened {
                pane: PaneId(7),
                request: RequestId(1),
            })
            .unwrap();
        events
            .send(OrzmuxEvent::PaneTitle {
                pane: PaneId(7),
                title: Some("zsh".into()),
            })
            .unwrap();
        events
            .send(OrzmuxEvent::PaneTitle {
                pane: PaneId(9),
                title: Some("gone".into()),
            })
            .unwrap();
        app.update();
        assert_eq!(
            app.world().get::<TtyTitle>(entity),
            Some(&TtyTitle(Some("zsh".into())))
        );
    }

    /// Asserts that `SelectionText` is forwarded as
    /// `TtySelectionTextSignal`, including a `None` answer.
    ///
    /// Case: the user copies from a pane that closed before the backend
    /// answered.
    #[test]
    fn selection_text_is_forwarded() {
        let (mut app, events) = app();
        events
            .send(OrzmuxEvent::SelectionText { text: None })
            .unwrap();
        app.update();
        assert_eq!(app.world().resource::<Seen>().texts, vec![None]);
    }

    /// Asserts that `SelectionCopied` reaches the host as a selection-text
    /// signal carrying the text.
    ///
    /// Case: the user finishes dragging a selection across a word, and the
    /// backend hands the text back for the clipboard.
    #[test]
    fn selection_copied_becomes_a_selection_text_signal() {
        let (mut app, events) = app();
        events
            .send(OrzmuxEvent::SelectionCopied { text: "hi".into() })
            .unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<Seen>().texts,
            vec![Some("hi".to_string())]
        );
    }

    /// Asserts that a vanished backend ends the session once and that the
    /// drain removes the connection in the frame that detects it.
    ///
    /// Case: the backend thread panicked.
    #[test]
    fn a_disconnected_backend_ends_the_session_once() {
        let (mut app, events) = app();
        drop(events);
        app.update();
        assert!(!app.world().contains_resource::<OrzmuxConnection>());
        assert_eq!(app.world().resource::<Seen>().ended, 1);
        app.update();
        assert_eq!(app.world().resource::<Seen>().ended, 1);
    }

    /// Asserts that a drain carrying only `Frame` events leaves
    /// `CurrentLayout` unchanged, while a `Layout` event marks it
    /// changed.
    ///
    /// Case: a running pane repaints every frame without the layout
    /// ever moving.
    #[test]
    fn a_frame_only_drain_does_not_change_the_layout() {
        let (mut app, events) = app();
        app.update();
        app.world_mut().resource_mut::<Seen>().layout_changes = 0;
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<PaneRegistry>()
            .panes
            .insert(PaneId(1), entity);
        events
            .send(OrzmuxEvent::Frame {
                pane: PaneId(1),
                frame: frame(GridSize::new(80, 24).expect("a valid size")),
            })
            .unwrap();
        app.update();
        assert_eq!(app.world().resource::<Seen>().layout_changes, 0);

        events
            .send(OrzmuxEvent::Layout {
                layout: layout(1, &[(PaneId(1), 0)]),
                frames: vec![],
            })
            .unwrap();
        app.update();
        assert_eq!(app.world().resource::<Seen>().layout_changes, 1);
    }
}
