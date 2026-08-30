//! Local VI applier: forwards each shared VI action event to the matching
//! `bevy_orzma_tty` request `EntityEvent`, and to the vi-mode exit event
//! `mode.rs` owns for selection toggling, yank, and exit.

use crate::action::clipboard::CopyAction;
use crate::action::vi::mode::ExitViMode;
use crate::action::vi::{
    ViExitRequest, ViMotionRequest, ViScrollRequest, ViSelectionToggleRequest, ViYankRequest,
};
use bevy::prelude::*;
use bevy_orzma_tty::prelude::{
    RequestTtyScroll, RequestTtySelectionClear, RequestTtySelectionKindChange,
    RequestTtySelectionStartAtViCursor, RequestTtyViMotion, SelectionKind,
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
    match resolve_selection_toggle(selection_type(), ev.ty) {
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

/// Copies the current selection (currently stubbed to nothing) and always
/// leaves vi mode.
fn on_vi_yank(ev: On<ViYankRequest>, mut commands: Commands) {
    if let Some(text) = selection_to_string() {
        commands.trigger(CopyAction { text });
    }
    commands.trigger(ExitViMode { entity: ev.entity });
}

/// Forwards a `ViExitRequest` as an `ExitViMode`.
fn on_vi_exit(ev: On<ViExitRequest>, mut commands: Commands) {
    commands.trigger(ExitViMode { entity: ev.entity });
}

/// Maps a `ViModeScroll` to the `Scroll` motion `bevy_orzma_tty` applies.
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

/// Resolves a selection toggle against the current selection: same kind
/// clears, a different kind switches, none starts.
fn resolve_selection_toggle(
    current: Option<SelectionKind>,
    requested: SelectionKind,
) -> SelectionOp {
    match current {
        Some(c) if c == requested => SelectionOp::Clear,
        Some(_) => SelectionOp::Change(requested),
        None => SelectionOp::Start(requested),
    }
}

// TODO: read the live selection kind from the VT once a selection
// capability exists (docs/todo/migrate-to-new-vt.md item 10); until then
// every toggle resolves to `SelectionOp::Start`.
fn selection_type() -> Option<SelectionKind> {
    None
}

// TODO: read the live selection text from the VT once a selection
// capability exists (docs/todo/migrate-to-new-vt.md item 10); until then
// yank never has anything to copy.
fn selection_to_string() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_orzma_tty::prelude::ViMotion;

    /// Asserts that a toggle starts a selection when none exists, clears
    /// one of the same kind, and switches one of a different kind.
    ///
    /// Case: the user presses `v`, then `v` again, then `V` in vi mode.
    #[test]
    fn selection_toggle_resolution() {
        assert_eq!(
            resolve_selection_toggle(None, SelectionKind::Simple),
            SelectionOp::Start(SelectionKind::Simple)
        );
        assert_eq!(
            resolve_selection_toggle(Some(SelectionKind::Simple), SelectionKind::Simple),
            SelectionOp::Clear
        );
        assert_eq!(
            resolve_selection_toggle(Some(SelectionKind::Simple), SelectionKind::Lines),
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

    /// Asserts that yank always exits vi mode and — since
    /// `selection_to_string` is stubbed to `None` until a selection-reading
    /// capability lands (docs/todo/migrate-to-new-vt.md item 10) — never
    /// emits a `CopyAction`.
    ///
    /// Case: the user presses the yank key in vi mode, with or without an
    /// active selection.
    #[test]
    fn yank_exits_vi_mode_and_currently_never_copies() {
        use crate::action::clipboard::test_support::{CapturedCopyActions, capture_copy_actions};

        let mut app = app_with_applier();
        app.init_resource::<SeenExits>().add_observer(
            |ev: On<ExitViMode>, mut seen: ResMut<SeenExits>| {
                seen.0.push(ev.entity);
            },
        );
        capture_copy_actions(&mut app);
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(ViYankRequest { entity });
        app.update();

        assert_eq!(app.world().resource::<SeenExits>().0, vec![entity]);
        assert!(app.world().resource::<CapturedCopyActions>().0.is_empty());
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
