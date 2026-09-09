//! `drain_orzmux_events`: empties the backend's event channel every frame
//! and turns each event into entity state or an `EntityEvent` the host
//! observes.

use crate::layout::CurrentLayout;
use crate::registry::PaneRegistry;
use crate::signals::{
    TtyChildExitSignal, TtyFrameSignal, TtySelectionTextSignal, trigger_vt_signal,
};
use crate::{OrzmuxConnection, OrzmuxPane, OrzmuxSystems};
use bevy::prelude::*;
use orzma_vt::prelude::Frame;
use orzmux::prelude::{CloseReason, OrzmuxEvent, PaneId};
use std::time::{Duration, Instant};

/// The session is over: the last pane closed, or the backend is gone.
#[derive(Event, Debug, Clone, Copy)]
pub struct OrzmuxSessionEnded;

/// A `NewPane` the backend refused; the host unbinds and despawns.
#[derive(EntityEvent, Debug, Clone)]
pub struct OrzmuxPaneSpawnFailed {
    #[event_target]
    pub entity: Entity,
    pub error: String,
}

/// Registers the drain.
pub(crate) struct DrainPlugin;

impl Plugin for DrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PaneRegistry>()
            .init_resource::<CurrentLayout>()
            .init_resource::<DisconnectReported>()
            .add_systems(Update, drain_orzmux_events.in_set(OrzmuxSystems::Drain));
    }
}

/// Whether `OrzmuxSessionEnded` was already triggered for a disconnect.
#[derive(Resource, Default)]
struct DisconnectReported(bool);

/// One `Update` that drained more frames than [`Self::THRESHOLD`],
/// with how long the drain took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DrainReport {
    /// Standalone frames plus the frames bundled in layouts.
    frames: usize,
    /// Wall time of the whole drain.
    elapsed: Duration,
}

impl DrainReport {
    /// A drain of more frames than this is reported.
    const THRESHOLD: usize = 8;

    /// The report for a drain of `frames` frames, or `None` when the
    /// count is within the threshold.
    fn over_threshold(frames: usize, elapsed: Duration) -> Option<Self> {
        (frames > Self::THRESHOLD).then_some(Self { frames, elapsed })
    }
}

/// Drains every queued event in order. Not gated on change detection:
/// channel arrivals are invisible to it. A drain of more than
/// [`DrainReport::THRESHOLD`] frames is logged under `orzmux::queues`
/// with its wall time.
fn drain_orzmux_events(
    mut commands: Commands,
    mut registry: ResMut<PaneRegistry>,
    mut current: ResMut<CurrentLayout>,
    mut reported: ResMut<DisconnectReported>,
    connection: Res<OrzmuxConnection>,
) {
    let started = Instant::now();
    let mut frames = 0;
    for event in connection.0.try_iter() {
        frames += frame_count(&event);
        apply_event(&mut commands, &mut registry, &mut current, event);
    }
    if connection.0.is_disconnected() && !reported.0 {
        reported.0 = true;
        commands.trigger(OrzmuxSessionEnded);
    }
    if let Some(report) = DrainReport::over_threshold(frames, started.elapsed()) {
        tracing::debug!(
            target: "orzmux::queues",
            frames = report.frames,
            elapsed = ?report.elapsed,
            "drained more frames than the threshold in one update"
        );
    }
}

/// `current` stays a `ResMut` so the write goes through `set_if_neq`:
/// dereferencing it mutably on every drain would mark the resource
/// changed on every frame and defeat `apply_layout`'s run condition.
fn apply_event(
    commands: &mut Commands,
    registry: &mut PaneRegistry,
    current: &mut ResMut<CurrentLayout>,
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
            if !current.0.panes.is_empty() && layout.panes.is_empty() {
                commands.trigger(OrzmuxSessionEnded);
            }
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
        OrzmuxEvent::SelectionText { text } => commands.trigger(TtySelectionTextSignal { text }),
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
    }
}

fn trigger_frame(commands: &mut Commands, registry: &PaneRegistry, pane: PaneId, frame: Frame) {
    match registry.entity_of(pane) {
        Some(terminal) => commands.trigger(TtyFrameSignal { terminal, frame }),
        None => tracing::debug!(?pane, "frame for an unknown pane dropped"),
    }
}

/// How many frames one event carries: one for `Frame`, the bundled
/// count for `Layout`, zero for everything else.
fn frame_count(event: &OrzmuxEvent) -> usize {
    match event {
        OrzmuxEvent::Frame { .. } => 1,
        OrzmuxEvent::Layout { frames, .. } => frames.len(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::app_with_channels;
    use crate::signals::TtyFrameSignal;
    use crossbeam_channel::Sender;
    use orzma_vt::prelude::{Cursor, DisplayOffset, GridSize, VtSignal};
    use orzmux::prelude::{CloseReason, CommandSeq, Layout, PaneRect, RequestId};

    #[derive(Resource, Default)]
    struct Seen {
        ended: usize,
        spawn_failed: Vec<(Entity, String)>,
        frames: Vec<Entity>,
        texts: Vec<Option<String>>,
        layout_changes: usize,
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
                (|current: Res<CurrentLayout>, mut seen: ResMut<Seen>| {
                    if current.is_changed() {
                        seen.layout_changes += 1;
                    }
                })
                .after(OrzmuxSystems::Drain),
            );
        (app, events)
    }

    fn frame(cols: u16, rows: u16) -> Frame {
        Frame {
            size: GridSize { cols, rows },
            rows: vec![],
            cursor: Cursor::default(),
            display_offset: DisplayOffset(0),
            vi_cursor: None,
            selection: None,
            placements: None,
            palette: None,
            hyperlinks: vec![],
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
                frame: frame(80, 24),
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

    /// Asserts that the session ends exactly when a non-empty layout is
    /// followed by an empty one, including within a single drain.
    ///
    /// Case: the only shell opens and exits before the GUI ran a frame.
    #[test]
    fn session_ends_on_the_non_empty_to_empty_transition_within_one_drain() {
        let (mut app, events) = app();
        events
            .send(OrzmuxEvent::Layout {
                layout: layout(1, &[(PaneId(1), 0)]),
                frames: vec![],
            })
            .unwrap();
        events
            .send(OrzmuxEvent::PaneClosed {
                pane: PaneId(1),
                reason: CloseReason::ChildExit { code: Some(0) },
            })
            .unwrap();
        events
            .send(OrzmuxEvent::Layout {
                layout: layout(2, &[]),
                frames: vec![],
            })
            .unwrap();
        app.update();
        assert_eq!(app.world().resource::<Seen>().ended, 1);
        events
            .send(OrzmuxEvent::Layout {
                layout: layout(3, &[]),
                frames: vec![],
            })
            .unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<Seen>().ended,
            1,
            "an already-empty layout does not end again"
        );
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

    /// Asserts that a vanished backend ends the session once.
    ///
    /// Case: the backend thread panicked.
    #[test]
    fn a_disconnected_backend_ends_the_session_once() {
        let (mut app, events) = app();
        drop(events);
        app.update();
        app.update();
        assert_eq!(app.world().resource::<Seen>().ended, 1);
    }

    /// Asserts that a drain carrying only `Frame` events leaves
    /// `CurrentLayout` unchanged, while a `Layout` event marks it
    /// changed.
    ///
    /// Case: a running pane repaints every frame without the layout
    /// ever moving, so a system gated on `Changed<CurrentLayout>` must
    /// not re-run on every repaint.
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
                frame: frame(80, 24),
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

    /// Asserts that the frame count sums standalone frames and the
    /// frames bundled in layouts, and that only a count above the
    /// threshold produces a report.
    ///
    /// Case: the GUI resumes after a live-resize stall with seven
    /// frames, a layout carrying two repaints, and a bell queued behind
    /// them.
    #[test]
    fn a_drain_over_the_frame_threshold_is_reported_and_one_within_it_is_not() {
        let mut events: Vec<OrzmuxEvent> = (0..7)
            .map(|_| OrzmuxEvent::Frame {
                pane: PaneId(1),
                frame: frame(80, 24),
            })
            .collect();
        events.push(OrzmuxEvent::Layout {
            layout: layout(1, &[(PaneId(1), 0), (PaneId(2), 10)]),
            frames: vec![(PaneId(1), frame(80, 24)), (PaneId(2), frame(80, 24))],
        });
        events.push(OrzmuxEvent::Signal {
            pane: PaneId(1),
            signal: VtSignal::Bell,
        });
        let frames: usize = events.iter().map(frame_count).sum();
        assert_eq!(frames, 9);
        let elapsed = Duration::from_millis(3);
        assert_eq!(
            DrainReport::over_threshold(frames, elapsed),
            Some(DrainReport { frames: 9, elapsed })
        );
        assert_eq!(DrainReport::over_threshold(8, elapsed), None);
    }
}
