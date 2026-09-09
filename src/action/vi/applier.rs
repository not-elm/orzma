//! Local VI applier: forwards each shared VI action event to the matching
//! `bevy_orzmux` request `EntityEvent`, and to the vi-mode exit event
//! `mode.rs` owns for selection toggling, yank, and exit.

use crate::action::vi::mode::ExitViMode;
use crate::action::vi::{
    ViExitRequest, ViMotionRequest, ViScrollRequest, ViSelectionToggleRequest, ViYankRequest,
};
use bevy::prelude::*;
use bevy_orzmux::prelude::{
    OrzmuxPane, RequestTtyCopySelection, RequestTtyScroll, RequestTtySelectionClear,
    RequestTtySelectionKindChange, RequestTtySelectionStartAtViCursor, RequestTtyViMotion,
    SelectionKind,
};
use orzma_configs::vi_mode::ViModeScroll;
use orzma_vt::prelude::Scroll;

/// Registers the local VI apply observers.
pub(super) struct ViApplierPlugin;

impl Plugin for ViApplierPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_vi_motion)
            .add_observer(on_vi_scroll)
            .add_observer(on_vi_selection_toggle)
            .add_observer(on_vi_yank)
            .add_observer(on_vi_exit);
    }
}

/// Forwards a `ViMotionRequest` as a `RequestTtyViMotion`.
fn on_vi_motion(ev: On<ViMotionRequest>, mut commands: Commands) {
    commands.trigger(RequestTtyViMotion {
        terminal: ev.entity,
        motion: ev.motion,
    });
}

/// Forwards a `ViScrollRequest` as a `RequestTtyScroll`.
fn on_vi_scroll(ev: On<ViScrollRequest>, mut commands: Commands) {
    commands.trigger(RequestTtyScroll {
        terminal: ev.entity,
        scroll: scroll_for(ev.kind),
    });
}

/// Resolves a selection toggle against the (currently stubbed) current
/// selection and requests the matching operation.
fn on_vi_selection_toggle(ev: On<ViSelectionToggleRequest>, mut commands: Commands) {
    match SelectionOp::resolve(selection_type(), ev.ty) {
        SelectionOp::Start(kind) => {
            commands.trigger(RequestTtySelectionStartAtViCursor {
                terminal: ev.entity,
                kind,
            });
        }
        SelectionOp::Change(kind) => {
            commands.trigger(RequestTtySelectionKindChange {
                terminal: ev.entity,
                kind,
            });
        }
        SelectionOp::Clear => {
            commands.trigger(RequestTtySelectionClear {
                terminal: ev.entity,
            });
        }
    }
}

/// Asks for the selection's text (answered later as a clipboard write)
/// and always leaves vi mode. FIFO on the backend keeps the copy ahead
/// of the exit's selection clear.
fn on_vi_yank(
    ev: On<ViYankRequest>,
    mut commands: Commands,
    terminals: Query<(), With<OrzmuxPane>>,
) {
    if terminals.get(ev.entity).is_ok() {
        commands.trigger(RequestTtyCopySelection {
            terminal: ev.entity,
        });
    }
    commands.trigger(ExitViMode { entity: ev.entity });
}

/// Forwards a `ViExitRequest` as an `ExitViMode`.
fn on_vi_exit(ev: On<ViExitRequest>, mut commands: Commands) {
    commands.trigger(ExitViMode { entity: ev.entity });
}

/// Maps a `ViModeScroll` to the `Scroll` motion `bevy_orzmux` applies.
fn scroll_for(kind: ViModeScroll) -> Scroll {
    match kind {
        ViModeScroll::PageUp => Scroll::PageUp,
        ViModeScroll::PageDown => Scroll::PageDown,
        ViModeScroll::HalfPageUp => Scroll::HalfPageUp,
        ViModeScroll::HalfPageDown => Scroll::HalfPageDown,
        ViModeScroll::ScrollUp => Scroll::Delta(1),
        ViModeScroll::ScrollDown => Scroll::Delta(-1),
        ViModeScroll::HistoryTop => Scroll::Top,
        ViModeScroll::HistoryBottom => Scroll::Bottom,
    }
}

/// A resolved selection-toggle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionOp {
    Start(SelectionKind),
    Change(SelectionKind),
    Clear,
}

impl SelectionOp {
    /// Resolves a selection toggle against the current selection: same kind
    /// clears, a different kind switches, none starts.
    fn resolve(current: Option<SelectionKind>, requested: SelectionKind) -> Self {
        match current {
            Some(c) if c == requested => Self::Clear,
            Some(_) => Self::Change(requested),
            None => Self::Start(requested),
        }
    }
}

// TODO: Read the live selection kind from the VT once vi mode lands there;
// until then every toggle resolves to `SelectionOp::Start`.
fn selection_type() -> Option<SelectionKind> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_orzmux::prelude::ViMotion;

    /// Asserts that a toggle starts a selection when none exists, clears
    /// one of the same kind, and switches one of a different kind.
    ///
    /// Case: the user presses `v`, then `v` again, then `V` in vi mode.
    #[test]
    fn selection_toggle_resolution() {
        assert_eq!(
            SelectionOp::resolve(None, SelectionKind::Simple),
            SelectionOp::Start(SelectionKind::Simple)
        );
        assert_eq!(
            SelectionOp::resolve(Some(SelectionKind::Simple), SelectionKind::Simple),
            SelectionOp::Clear
        );
        assert_eq!(
            SelectionOp::resolve(Some(SelectionKind::Simple), SelectionKind::Lines),
            SelectionOp::Change(SelectionKind::Lines)
        );
    }

    /// Asserts that every `ViModeScroll` maps to the intended `Scroll`
    /// motion, with line scrolls as one-line deltas.
    ///
    /// Case: the user presses each configured vi-mode scroll key in turn.
    #[test]
    fn scroll_for_maps_every_vi_mode_scroll_kind() {
        assert_eq!(scroll_for(ViModeScroll::PageUp), Scroll::PageUp);
        assert_eq!(scroll_for(ViModeScroll::PageDown), Scroll::PageDown);
        assert_eq!(scroll_for(ViModeScroll::HalfPageUp), Scroll::HalfPageUp);
        assert_eq!(scroll_for(ViModeScroll::HalfPageDown), Scroll::HalfPageDown);
        assert_eq!(scroll_for(ViModeScroll::ScrollUp), Scroll::Delta(1));
        assert_eq!(scroll_for(ViModeScroll::ScrollDown), Scroll::Delta(-1));
        assert_eq!(scroll_for(ViModeScroll::HistoryTop), Scroll::Top);
        assert_eq!(scroll_for(ViModeScroll::HistoryBottom), Scroll::Bottom);
    }

    fn app_with_applier() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(ViApplierPlugin);
        app
    }

    #[derive(Resource, Default)]
    struct SeenMotions(Vec<(Entity, ViMotion)>);

    /// Asserts that `ViMotionRequest` is forwarded as a `RequestTtyViMotion`
    /// carrying the same entity and motion.
    ///
    /// Case: a vi-mode motion key (`j`) resolved by the keymap and fired at
    /// the focused terminal.
    #[test]
    fn vi_motion_triggers_the_matching_request() {
        let mut app = app_with_applier();
        app.init_resource::<SeenMotions>().add_observer(
            |ev: On<RequestTtyViMotion>, mut seen: ResMut<SeenMotions>| {
                seen.0.push((ev.terminal, ev.motion));
            },
        );
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(ViMotionRequest {
            entity,
            motion: ViMotion::Down,
        });
        app.update();

        assert_eq!(
            app.world().resource::<SeenMotions>().0,
            vec![(entity, ViMotion::Down)]
        );
    }

    #[derive(Resource, Default)]
    struct SeenScrolls(Vec<(Entity, Scroll)>);

    /// Asserts that `ViScrollRequest` is forwarded as a `RequestTtyScroll`
    /// through `scroll_for`'s mapping.
    ///
    /// Case: the user pages through scrollback with `Ctrl-F` in vi mode.
    #[test]
    fn vi_scroll_triggers_the_matching_request() {
        let mut app = app_with_applier();
        app.init_resource::<SeenScrolls>().add_observer(
            |ev: On<RequestTtyScroll>, mut seen: ResMut<SeenScrolls>| {
                seen.0.push((ev.terminal, ev.scroll));
            },
        );
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(ViScrollRequest {
            entity,
            kind: ViModeScroll::PageDown,
        });
        app.update();

        assert_eq!(
            app.world().resource::<SeenScrolls>().0,
            vec![(entity, Scroll::PageDown)]
        );
    }

    #[derive(Resource, Default)]
    struct SeenStarts(Vec<(Entity, SelectionKind)>);

    /// Asserts that a selection toggle — always resolving to `Start` while
    /// `selection_type` is stubbed to `None` — triggers
    /// `RequestTtySelectionStartAtViCursor` with the requested kind.
    ///
    /// Case: the user presses `v` in vi mode with no selection active.
    #[test]
    fn vi_selection_toggle_starts_at_the_vi_cursor() {
        let mut app = app_with_applier();
        app.init_resource::<SeenStarts>().add_observer(
            |ev: On<RequestTtySelectionStartAtViCursor>, mut seen: ResMut<SeenStarts>| {
                seen.0.push((ev.terminal, ev.kind));
            },
        );
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(ViSelectionToggleRequest {
            entity,
            ty: SelectionKind::Lines,
        });
        app.update();

        assert_eq!(
            app.world().resource::<SeenStarts>().0,
            vec![(entity, SelectionKind::Lines)]
        );
    }

    #[derive(Resource, Default)]
    struct SeenExits(Vec<Entity>);

    /// Asserts that a yank on an entity without an `OrzmuxPane` still exits vi
    /// mode, requesting no copy.
    ///
    /// Case: the yank key lands while the focused pane is being torn down.
    #[test]
    fn yank_without_a_pane_exits_vi_mode_and_requests_no_copy() {
        #[derive(Resource, Default)]
        struct Order(Vec<&'static str>);
        let mut app = app_with_applier();
        app.init_resource::<Order>()
            .add_observer(|_: On<RequestTtyCopySelection>, mut o: ResMut<Order>| o.0.push("copy"))
            .add_observer(|_: On<ExitViMode>, mut o: ResMut<Order>| o.0.push("exit"));
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(ViYankRequest { entity });
        app.update();

        assert_eq!(app.world().resource::<Order>().0, vec!["exit"]);
    }

    /// Asserts that a yank asks for the selection text before leaving vi
    /// mode, so the backend copies before the exit's selection clear.
    ///
    /// Case: the user presses `y` on a vi-mode selection.
    #[test]
    fn yank_requests_the_copy_then_exits_vi_mode() {
        use crate::surface::OrzmaTerminal;
        use orzmux::prelude::PaneId;

        #[derive(Resource, Default)]
        struct Order(Vec<&'static str>);
        let mut app = app_with_applier();
        app.init_resource::<Order>()
            .add_observer(|_: On<RequestTtyCopySelection>, mut o: ResMut<Order>| o.0.push("copy"))
            .add_observer(|_: On<ExitViMode>, mut o: ResMut<Order>| o.0.push("exit"));
        let entity = app
            .world_mut()
            .spawn((OrzmaTerminal, OrzmuxPane(PaneId(1))))
            .id();
        app.world_mut().trigger(ViYankRequest { entity });
        app.update();
        assert_eq!(app.world().resource::<Order>().0, vec!["copy", "exit"]);
    }

    /// Asserts that a `ViExitRequest` always triggers `ExitViMode`, even for
    /// an entity without a terminal handle, rather than gating locally.
    ///
    /// Case: an exit key is pressed while the target pane is mid-teardown.
    #[test]
    fn vi_exit_always_triggers_exit_vi_mode() {
        let mut app = app_with_applier();
        app.init_resource::<SeenExits>().add_observer(
            |ev: On<ExitViMode>, mut seen: ResMut<SeenExits>| {
                seen.0.push(ev.entity);
            },
        );
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(ViExitRequest { entity });
        app.update();

        assert_eq!(app.world().resource::<SeenExits>().0, vec![entity]);
    }
}
