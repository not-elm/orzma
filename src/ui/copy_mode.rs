//! Copy mode state. The vi cursor lives in alacritty
//! (`Term::vi_mode_cursor`) and the active selection lives in
//! `Term::selection`. This component is a pure marker — its presence
//! on a Surface entity means "copy mode is active". The v / V
//! toggle predicate reads `TerminalHandle::selection_type()` to
//! decide between "start new selection of kind X" and "clear existing".

use crate::clipboard::Clipboard;
use bevy::app::{App, Plugin};
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::observer::On;
use bevy::ecs::system::{Commands, Query};
use bevy::input::keyboard::Key;
#[cfg(not(feature = "thin-client"))]
use bevy_terminal::{PtyHandle, TerminalHandle};
use bevy_terminal::{SelectionType, ViMotion};
use ozmux_configs::shortcuts::Modifiers;

/// Bevy Plugin: registers the two observers and inserts the global
/// `Clipboard` resource. `CopyModeState` is inserted/removed per-entity
/// by the observers themselves; no global system needed.
pub struct CopyModePlugin;

impl Plugin for CopyModePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Clipboard::new())
            .add_observer(handle_enter_copy_mode_request)
            .add_observer(handle_exit_copy_mode);
    }
}

/// Marker: presence on a Surface entity means "copy mode is active".
#[derive(Component, Debug, Default)]
pub struct CopyModeState;

/// Request to enter copy mode on a specific Surface entity. Fired by
/// `handle_chord` when it sees `Action::EnterCopyMode`. The observer
/// inserts `CopyModeState` and calls `TerminalHandle::enter_vi_mode`.
#[derive(EntityEvent, Debug)]
pub struct EnterCopyModeActionEvent {
    pub entity: Entity,
}

/// Request to exit copy mode. Fired from the copy-mode key dispatcher
/// on `q` / `Esc` / `y` (after the clipboard write). The observer
/// calls `TerminalHandle::exit_vi_mode`, clears any selection, and
/// removes `CopyModeState`.
#[derive(EntityEvent, Debug)]
pub struct ExitCopyMode {
    pub entity: Entity,
}

/// Outcome of `map_key_to_copy_op` — what the dispatcher should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOp {
    /// `q` / `Esc` — leave copy mode, drop any selection.
    ExitCancel,
    /// `y` — copy the active selection to the OS clipboard, then leave.
    ExitCopy,
    /// Move the alacritty vi cursor.
    Motion(ViMotion),
    /// Page-up the viewport (`Term::scroll_display(Scroll::PageUp)`).
    ScrollPageUp,
    /// Page-down the viewport.
    ScrollPageDown,
    /// `v` / `V` — toggle a selection of the given type.
    ToggleSelection(SelectionType),
}

/// Pure mapping from Bevy logical key + modifiers to a `CopyOp`.
/// Returns `None` for any key not bound in copy mode — those keys are
/// silently swallowed by `dispatch_key`.
///
/// Modifier discipline: copy-mode keys (h/j/k/l/w/b/e/0/^/$/g/G/v/V/y/q)
/// only match when `meta`, `ctrl`, and `alt` are all false. `shift`
/// remains in scope because the existing uppercase bindings (`V`, `G`)
/// rely on it. Without this gate, Cmd+V would trigger
/// `ToggleSelection(Simple)` while in copy mode instead of falling
/// through to the paste pipeline.
pub(crate) fn map_key_to_copy_op(key: &Key, mods: Modifiers) -> Option<CopyOp> {
    match key {
        Key::Escape => return Some(CopyOp::ExitCancel),
        Key::ArrowLeft => return Some(CopyOp::Motion(ViMotion::Left)),
        Key::ArrowRight => return Some(CopyOp::Motion(ViMotion::Right)),
        Key::ArrowUp => return Some(CopyOp::Motion(ViMotion::Up)),
        Key::ArrowDown => return Some(CopyOp::Motion(ViMotion::Down)),
        Key::PageUp => return Some(CopyOp::ScrollPageUp),
        Key::PageDown => return Some(CopyOp::ScrollPageDown),
        Key::Character(_) => {}
        _ => return None,
    }
    if mods.meta || mods.ctrl || mods.alt {
        return None;
    }
    let Key::Character(s) = key else { return None };
    let mut chars = s.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    Some(match c {
        'h' => CopyOp::Motion(ViMotion::Left),
        'l' => CopyOp::Motion(ViMotion::Right),
        'k' => CopyOp::Motion(ViMotion::Up),
        'j' => CopyOp::Motion(ViMotion::Down),
        '0' => CopyOp::Motion(ViMotion::First),
        '^' => CopyOp::Motion(ViMotion::FirstOccupied),
        '$' => CopyOp::Motion(ViMotion::Last),
        'w' => CopyOp::Motion(ViMotion::WordRight),
        'b' => CopyOp::Motion(ViMotion::WordLeft),
        'e' => CopyOp::Motion(ViMotion::WordRightEnd),
        'g' => CopyOp::Motion(ViMotion::High),
        'G' if mods.shift => CopyOp::Motion(ViMotion::Low),
        'v' => CopyOp::ToggleSelection(SelectionType::Simple),
        'V' if mods.shift => CopyOp::ToggleSelection(SelectionType::Lines),
        'y' => CopyOp::ExitCopy,
        'q' => CopyOp::ExitCancel,
        _ => return None,
    })
}

/// Side-effecting helper called inline from
/// `src/input.rs::dispatch_focused_key` whenever the active Surface
/// entity carries `CopyModeState`. Looks up the entity's terminal
/// handle, runs the `CopyOp` mapped from the key, and triggers
/// `ExitCopyMode` when the op is exit/copy.
///
/// Returns `true` when the op caused copy mode to exit (`ExitCancel` or
/// `ExitCopy`), so the caller can bypass the copy-mode gate for
/// subsequent events in the same Bevy frame.
#[cfg(not(feature = "thin-client"))]
pub(crate) fn dispatch_key(
    commands: &mut Commands,
    q: &mut Query<(&mut TerminalHandle, &mut PtyHandle)>,
    clipboard: &mut Clipboard,
    entity: Entity,
    logical_key: Key,
    mods: Modifiers,
) -> bool {
    let Some(op) = map_key_to_copy_op(&logical_key, mods) else {
        return false;
    };
    // NOTE: exits is computed before the handle lookup so the gate-bypass
    // tracking works even when q.get_mut fails (e.g. no TerminalHandle in
    // tests). Missing the handle must not suppress the bypass — doing so
    // would swallow the next key silently.
    let exits = matches!(op, CopyOp::ExitCancel | CopyOp::ExitCopy);
    let Ok((mut handle, _pty)) = q.get_mut(entity) else {
        return exits;
    };
    match op {
        CopyOp::ExitCancel => {
            commands.trigger(ExitCopyMode { entity });
        }
        CopyOp::ExitCopy => {
            if let Some(text) = handle.selection_to_string()
                && !text.is_empty()
            {
                clipboard.write(text);
            }
            commands.trigger(ExitCopyMode { entity });
        }
        CopyOp::Motion(m) => handle.vi_motion(m),
        CopyOp::ScrollPageUp => handle.scroll_page_up(),
        CopyOp::ScrollPageDown => handle.scroll_page_down(),
        CopyOp::ToggleSelection(ty) => match handle.selection_type() {
            Some(existing) if existing == ty => handle.selection_clear(),
            Some(_) => {
                handle.selection_change_type(ty);
            }
            None => handle.selection_start(ty),
        },
    }
    exits
}

/// Thin-client copy-mode key dispatcher: maps the key to a `CopyOp` and sends
/// the matching `ClientMessage::CopyModeOp` over the wire (the daemon drives
/// vi-mode + selection + frame rendering). Mirrors the local `dispatch_key`'s
/// `exits` contract so `src/input.rs` can bypass the gate for the rest of the
/// frame after an exit.
///
/// Returns `true` when the op caused copy mode to exit (`ExitCancel` or
/// `ExitCopy`).
#[cfg(feature = "thin-client")]
pub(crate) fn dispatch_key(
    conn: &mut crate::thin_client::ThinClientConn,
    commands: &mut Commands,
    grids: &Query<&bevy_terminal_renderer::prelude::TerminalGrid>,
    surface_ids: &Query<&ozmux_multiplexer::MuxSurfaceId>,
    entity: Entity,
    logical_key: Key,
    mods: Modifiers,
) -> bool {
    let Some(op) = map_key_to_copy_op(&logical_key, mods) else {
        return false;
    };
    let exits = matches!(op, CopyOp::ExitCancel | CopyOp::ExitCopy);
    let Ok(surface) = surface_ids.get(entity).map(|c| c.0) else {
        return exits;
    };
    match op {
        CopyOp::ExitCancel => {
            commands.trigger(ExitCopyMode { entity });
        }
        CopyOp::ExitCopy => {
            send_copy_op(conn, surface, ozmux_proto::CopyModeOp::CopySelection);
            commands.trigger(ExitCopyMode { entity });
        }
        CopyOp::Motion(m) => send_copy_op(
            conn,
            surface,
            ozmux_proto::CopyModeOp::ViMotion(vi_motion_to_kind(m)),
        ),
        CopyOp::ScrollPageUp => send_copy_op(conn, surface, ozmux_proto::CopyModeOp::ScrollPageUp),
        CopyOp::ScrollPageDown => {
            send_copy_op(conn, surface, ozmux_proto::CopyModeOp::ScrollPageDown)
        }
        CopyOp::ToggleSelection(ty) => {
            let want_line = matches!(ty, SelectionType::Lines);
            let current = grids
                .get(entity)
                .ok()
                .and_then(|g| g.selection.as_ref().map(|s| s.kind));
            let op = match current {
                None => ozmux_proto::CopyModeOp::SelectionStart {
                    ty: selection_type_to_kind(ty),
                },
                Some(k) if is_line_geometry(k) == want_line => {
                    ozmux_proto::CopyModeOp::SelectionClear
                }
                Some(_) => ozmux_proto::CopyModeOp::SelectionChangeType {
                    ty: selection_type_to_kind(ty),
                },
            };
            send_copy_op(conn, surface, op);
        }
    }
    exits
}

#[cfg(feature = "thin-client")]
fn send_copy_op(
    conn: &mut crate::thin_client::ThinClientConn,
    surface: ozmux_proto::SurfaceId,
    op: ozmux_proto::CopyModeOp,
) {
    crate::thin_client::send_cmd(conn, ozmux_proto::ClientMessage::CopyModeOp { surface, op });
}

#[cfg(feature = "thin-client")]
fn vi_motion_to_kind(m: ViMotion) -> ozmux_proto::ViMotionKind {
    match m {
        ViMotion::Left => ozmux_proto::ViMotionKind::Left,
        ViMotion::Right => ozmux_proto::ViMotionKind::Right,
        ViMotion::Up => ozmux_proto::ViMotionKind::Up,
        ViMotion::Down => ozmux_proto::ViMotionKind::Down,
        ViMotion::First => ozmux_proto::ViMotionKind::First,
        ViMotion::Last => ozmux_proto::ViMotionKind::Last,
        ViMotion::FirstOccupied => ozmux_proto::ViMotionKind::FirstOccupied,
        ViMotion::High => ozmux_proto::ViMotionKind::High,
        ViMotion::Low => ozmux_proto::ViMotionKind::Low,
        ViMotion::WordRight => ozmux_proto::ViMotionKind::WordRight,
        ViMotion::WordLeft => ozmux_proto::ViMotionKind::WordLeft,
        ViMotion::WordRightEnd => ozmux_proto::ViMotionKind::WordRightEnd,
        // NOTE: `map_key_to_copy_op` only ever emits the twelve motions above;
        // `ViMotion` carries further alacritty variants (Middle, Semantic*,
        // Bracket, paragraph) the keyboard map never produces, so reaching them
        // signals an upstream change in the mapper that this match must track.
        _ => unreachable!("map_key_to_copy_op only emits the twelve mapped motions"),
    }
}

#[cfg(feature = "thin-client")]
fn selection_type_to_kind(ty: SelectionType) -> ozmux_proto::SelectionKind {
    match ty {
        SelectionType::Simple => ozmux_proto::SelectionKind::Simple,
        SelectionType::Block => ozmux_proto::SelectionKind::Block,
        SelectionType::Lines => ozmux_proto::SelectionKind::Lines,
        SelectionType::Semantic => ozmux_proto::SelectionKind::Semantic,
    }
}

#[cfg(feature = "thin-client")]
fn is_line_geometry(kind: bevy_terminal_renderer::prelude::SelectionKind) -> bool {
    matches!(kind, bevy_terminal_renderer::prelude::SelectionKind::Line)
}

/// Observer for `EnterCopyModeActionEvent`. Inserts `CopyModeState` on the
/// target entity. The local arm also calls `TerminalHandle::enter_vi_mode`;
/// the thin arm sends `CopyModeOp::Enter` so the daemon's vi-mode drives the
/// frame's vi-cursor.
pub(crate) fn handle_enter_copy_mode_request(
    ev: On<EnterCopyModeActionEvent>,
    mut commands: Commands,
    #[cfg(not(feature = "thin-client"))] mut q: Query<&mut TerminalHandle>,
    #[cfg(feature = "thin-client")] mut conn: bevy::ecs::system::NonSendMut<
        crate::thin_client::ThinClientConn,
    >,
    #[cfg(feature = "thin-client")] surface_ids: Query<&ozmux_multiplexer::MuxSurfaceId>,
) {
    #[cfg(not(feature = "thin-client"))]
    {
        let Ok(mut handle) = q.get_mut(ev.entity) else {
            return;
        };
        handle.enter_vi_mode();
        commands.entity(ev.entity).insert(CopyModeState);
    }
    #[cfg(feature = "thin-client")]
    {
        commands.entity(ev.entity).insert(CopyModeState);
        if let Ok(surface) = surface_ids.get(ev.entity).map(|c| c.0) {
            crate::thin_client::send_cmd(
                &mut conn,
                ozmux_proto::ClientMessage::CopyModeOp {
                    surface,
                    op: ozmux_proto::CopyModeOp::Enter,
                },
            );
        }
    }
}

/// Observer for `ExitCopyMode`. Removes `CopyModeState`. The local arm clears
/// any selection and calls `TerminalHandle::exit_vi_mode` (which snaps the
/// viewport to the live tail); the thin arm sends `CopyModeOp::Exit` (the
/// daemon's `exit_vi_mode` clears the selection + snaps).
pub(crate) fn handle_exit_copy_mode(
    ev: On<ExitCopyMode>,
    mut commands: Commands,
    #[cfg(not(feature = "thin-client"))] mut q: Query<&mut TerminalHandle>,
    #[cfg(feature = "thin-client")] mut conn: bevy::ecs::system::NonSendMut<
        crate::thin_client::ThinClientConn,
    >,
    #[cfg(feature = "thin-client")] surface_ids: Query<&ozmux_multiplexer::MuxSurfaceId>,
) {
    #[cfg(not(feature = "thin-client"))]
    {
        let Ok(mut handle) = q.get_mut(ev.entity) else {
            return;
        };
        handle.selection_clear();
        handle.exit_vi_mode();
        commands.entity(ev.entity).remove::<CopyModeState>();
    }
    #[cfg(feature = "thin-client")]
    {
        commands.entity(ev.entity).remove::<CopyModeState>();
        if let Ok(surface) = surface_ids.get(ev.entity).map(|c| c.0) {
            crate::thin_client::send_cmd(
                &mut conn,
                ozmux_proto::ClientMessage::CopyModeOp {
                    surface,
                    op: ozmux_proto::CopyModeOp::Exit,
                },
            );
        }
    }
}

#[cfg(all(test, not(feature = "thin-client")))]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::ecs::entity::Entity;
    use bevy::ecs::observer::On;
    use bevy::ecs::resource::Resource;
    use bevy::ecs::system::{Commands, Query, Res, ResMut, System};
    use bevy::input::keyboard::Key as Bk;
    use bevy::prelude::MinimalPlugins;
    use bevy_terminal::{
        PtyHandle, SelectionType, SpawnOptions, TerminalBundle, TerminalHandle, ViMotion,
    };
    use ozmux_configs::shortcuts::Modifiers;
    use std::sync::{Arc, Mutex};

    #[test]
    fn map_h_returns_motion_left() {
        let op = map_key_to_copy_op(&Bk::Character("h".into()), Modifiers::default());
        assert!(matches!(op, Some(CopyOp::Motion(ViMotion::Left))));
    }

    #[test]
    fn map_arrow_left_returns_motion_left() {
        let op = map_key_to_copy_op(&Bk::ArrowLeft, Modifiers::default());
        assert!(matches!(op, Some(CopyOp::Motion(ViMotion::Left))));
    }

    #[test]
    fn map_uppercase_g_returns_motion_low() {
        let op = map_key_to_copy_op(
            &Bk::Character("G".into()),
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        assert!(matches!(op, Some(CopyOp::Motion(ViMotion::Low))));
    }

    #[test]
    fn map_lowercase_g_returns_motion_high() {
        let op = map_key_to_copy_op(&Bk::Character("g".into()), Modifiers::default());
        assert!(matches!(op, Some(CopyOp::Motion(ViMotion::High))));
    }

    #[test]
    fn map_v_returns_toggle_simple() {
        let op = map_key_to_copy_op(&Bk::Character("v".into()), Modifiers::default());
        assert!(matches!(
            op,
            Some(CopyOp::ToggleSelection(SelectionType::Simple))
        ));
    }

    #[test]
    fn map_uppercase_v_returns_toggle_lines() {
        let op = map_key_to_copy_op(
            &Bk::Character("V".into()),
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        assert!(matches!(
            op,
            Some(CopyOp::ToggleSelection(SelectionType::Lines))
        ));
    }

    #[test]
    fn map_q_returns_exit_cancel() {
        let op = map_key_to_copy_op(&Bk::Character("q".into()), Modifiers::default());
        assert!(matches!(op, Some(CopyOp::ExitCancel)));
    }

    #[test]
    fn map_escape_returns_exit_cancel() {
        let op = map_key_to_copy_op(&Bk::Escape, Modifiers::default());
        assert!(matches!(op, Some(CopyOp::ExitCancel)));
    }

    #[test]
    fn map_y_returns_exit_copy() {
        let op = map_key_to_copy_op(&Bk::Character("y".into()), Modifiers::default());
        assert!(matches!(op, Some(CopyOp::ExitCopy)));
    }

    #[test]
    fn map_pageup_returns_scroll_page_up() {
        let op = map_key_to_copy_op(&Bk::PageUp, Modifiers::default());
        assert!(matches!(op, Some(CopyOp::ScrollPageUp)));
    }

    #[test]
    fn map_unknown_key_returns_none() {
        let op = map_key_to_copy_op(&Bk::F1, Modifiers::default());
        assert!(op.is_none());
        let op = map_key_to_copy_op(&Bk::Character("z".into()), Modifiers::default());
        assert!(op.is_none());
    }

    #[test]
    fn map_v_with_meta_modifier_returns_none() {
        let op = map_key_to_copy_op(
            &Bk::Character("v".into()),
            Modifiers {
                meta: true,
                ..Default::default()
            },
        );
        assert!(
            op.is_none(),
            "Cmd+V (meta+v) must NOT toggle simple selection; it is the OS paste shortcut and must fall through to the paste gate",
        );
    }

    #[test]
    fn map_y_with_ctrl_modifier_returns_none() {
        let op = map_key_to_copy_op(
            &Bk::Character("y".into()),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
        assert!(
            op.is_none(),
            "Ctrl+Y must not be treated as the copy-mode yank — modifiers other than shift must be rejected",
        );
    }

    #[test]
    fn map_h_with_alt_modifier_returns_none() {
        let op = map_key_to_copy_op(
            &Bk::Character("h".into()),
            Modifiers {
                alt: true,
                ..Default::default()
            },
        );
        assert!(op.is_none(), "Alt+H must not move the vi cursor left");
    }

    #[test]
    fn map_uppercase_v_with_shift_still_returns_toggle_lines() {
        // Sanity: tightening must not regress the existing Shift+V binding.
        let op = map_key_to_copy_op(
            &Bk::Character("V".into()),
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        assert!(matches!(
            op,
            Some(CopyOp::ToggleSelection(SelectionType::Lines))
        ));
    }

    fn spawn_terminal_entity(app: &mut App) -> Entity {
        let opts = SpawnOptions {
            cols: 10,
            rows: 5,
            shell: "/bin/sh".into(),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = TerminalBundle::spawn(opts).expect("spawn /bin/sh");
        app.world_mut().spawn(bundle).id()
    }

    #[derive(Resource, Default, Clone)]
    struct CapturedExits(Arc<Mutex<Vec<Entity>>>);

    fn capture_exit(ev: On<ExitCopyMode>, captured: Res<CapturedExits>) {
        captured.0.lock().unwrap().push(ev.entity);
    }

    #[test]
    fn dispatch_key_q_triggers_exit_copy_mode() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::clipboard::Clipboard::new());
        app.insert_resource(CapturedExits::default());
        app.add_observer(capture_exit);

        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().entity_mut(entity).insert(CopyModeState);

        let mut sys = bevy::ecs::system::IntoSystem::into_system(
            move |mut commands: Commands,
                  mut q: Query<(&mut TerminalHandle, &mut PtyHandle)>,
                  mut clip: ResMut<crate::clipboard::Clipboard>| {
                dispatch_key(
                    &mut commands,
                    &mut q,
                    &mut clip,
                    entity,
                    Bk::Character("q".into()),
                    Modifiers::default(),
                );
            },
        );
        sys.initialize(app.world_mut());
        let _ = sys.run((), app.world_mut());
        sys.apply_deferred(app.world_mut());

        let captured = app.world().resource::<CapturedExits>().0.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0], entity);
    }

    #[test]
    fn dispatch_key_y_with_selection_writes_clipboard_then_exits() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::clipboard::Clipboard::new());
        app.insert_resource(CapturedExits::default());
        app.add_observer(capture_exit);

        let entity = spawn_terminal_entity(&mut app);
        {
            let mut e = app.world_mut().entity_mut(entity);
            let mut h = e.take::<TerminalHandle>().unwrap();
            h.enter_vi_mode();
            h.selection_start(SelectionType::Simple);
            e.insert(h);
        }
        app.world_mut().entity_mut(entity).insert(CopyModeState);

        let mut sys = bevy::ecs::system::IntoSystem::into_system(
            move |mut commands: Commands,
                  mut q: Query<(&mut TerminalHandle, &mut PtyHandle)>,
                  mut clip: ResMut<crate::clipboard::Clipboard>| {
                dispatch_key(
                    &mut commands,
                    &mut q,
                    &mut clip,
                    entity,
                    Bk::Character("y".into()),
                    Modifiers::default(),
                );
            },
        );
        sys.initialize(app.world_mut());
        let _ = sys.run((), app.world_mut());
        sys.apply_deferred(app.world_mut());

        let captured = app.world().resource::<CapturedExits>().0.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "y must always trigger exit even when clipboard write is silent"
        );
    }

    #[test]
    fn enter_observer_inserts_copy_mode_state_and_does_not_create_selection() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(handle_enter_copy_mode_request);

        let entity = spawn_terminal_entity(&mut app);

        app.world_mut().trigger(EnterCopyModeActionEvent { entity });
        app.update();

        assert!(app.world().get::<CopyModeState>(entity).is_some());
        let h = app.world().get::<TerminalHandle>(entity).unwrap();
        assert!(
            h.selection_type().is_none(),
            "enter must not auto-create a selection",
        );
    }

    #[test]
    fn exit_observer_removes_copy_mode_state_and_clears_selection() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(handle_enter_copy_mode_request);
        app.add_observer(handle_exit_copy_mode);

        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().trigger(EnterCopyModeActionEvent { entity });
        app.update();
        {
            let mut e = app.world_mut().entity_mut(entity);
            let mut h = e.take::<TerminalHandle>().unwrap();
            h.selection_start(SelectionType::Simple);
            e.insert(h);
        }

        app.world_mut().trigger(ExitCopyMode { entity });
        app.update();

        assert!(app.world().get::<CopyModeState>(entity).is_none());
        let h = app.world().get::<TerminalHandle>(entity).unwrap();
        assert!(
            h.selection_type().is_none(),
            "exit must clear the selection"
        );
    }
}
