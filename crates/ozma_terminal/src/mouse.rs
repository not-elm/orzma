//! Mouse-effect apply path for the Ozma terminal: the shared
//! `TerminalMouseEffects` / `TerminalForwardInput` events, the `MouseEffect`
//! intent type, and the apply observer (`on_terminal_mouse_effects`) that writes
//! the decided effects to the `TerminalHandle` / `Clipboard` (or forwards them to
//! a PTY-less backend). The mode-neutral mouse dispatch that DECIDES these
//! effects lives in the host (`crate::input::mouse` in the binary), scheduled
//! in `InputPhase::Dispatch`.

use crate::clipboard::Clipboard;
use crate::hyperlink::try_open_uri;
use crate::spawn::OzmaTerminal;
use bevy::prelude::*;
use ozma_tty_engine::{Coalescer, Point, PtyHandle, SelectionType, Side, TerminalHandle};

/// A resolved intent the apply step writes to the handle / clipboard. Kept
/// separate from application so the decision logic is unit-testable without a
/// `TerminalHandle` (which has no public constructor).
#[derive(Debug, Clone, PartialEq)]
pub enum MouseEffect {
    /// Write these bytes to the PTY.
    Write(Vec<u8>),
    /// Start a new local selection at `point`.
    SelStart {
        point: Point,
        side: Side,
        ty: SelectionType,
    },
    /// Extend the current selection's moving end to `point`.
    SelUpdate { point: Point, side: Side },
    /// Clear any active local selection.
    SelClear,
    /// Copy the current selection to the clipboard.
    Copy,
    /// Scroll the viewport by `i32` lines (negative = up).
    Scroll(i32),
    /// Open the given URI in the host browser / handler.
    OpenUri(String),
}

/// Terminal input bytes destined for the backend of `entity` (a PTY for a
/// local terminal, or tmux `send-keys` for a control-mode pane). Emitted by the
/// mouse apply observer when the terminal has no `PtyHandle`; the host owns the
/// observer that routes it to the real backend.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalForwardInput {
    /// The terminal entity whose backend should receive `bytes`.
    #[event_target]
    pub entity: Entity,
    /// The raw bytes to deliver to the backend.
    pub bytes: Vec<u8>,
}

/// Writes mouse-protocol report bytes to `entity`'s backend (PTY when
/// attached, `TerminalForwardInput` when detached).
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalMouseWrite {
    /// The terminal entity whose backend receives `bytes`.
    #[event_target]
    pub entity: Entity,
    /// The report bytes to deliver.
    pub bytes: Vec<u8>,
}

/// Starts a new local selection on `entity` at `point`.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalSelectionStart {
    /// The terminal entity to start the selection on.
    #[event_target]
    pub entity: Entity,
    /// The viewport-relative anchor of the new selection.
    pub point: Point,
    /// Which half of the cell the anchor sits in.
    pub side: Side,
    /// The selection granularity (simple / semantic / lines).
    pub ty: SelectionType,
}

/// Extends `entity`'s current selection's moving end to `point`.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalSelectionUpdate {
    /// The terminal entity whose selection is extended.
    #[event_target]
    pub entity: Entity,
    /// The viewport-relative moving end.
    pub point: Point,
    /// Which half of the cell the moving end sits in.
    pub side: Side,
}

/// Clears any active local selection on `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalSelectionClear {
    /// The terminal entity whose selection is cleared.
    #[event_target]
    pub entity: Entity,
}

/// Copies `entity`'s current selection to the clipboard.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalSelectionCopy {
    /// The terminal entity whose selection is copied.
    #[event_target]
    pub entity: Entity,
}

/// Scrolls `entity`'s viewport by `lines` (negative = up / into history).
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalViewportScroll {
    /// The terminal entity to scroll.
    #[event_target]
    pub entity: Entity,
    /// Lines to scroll; negative scrolls up into scrollback.
    pub lines: i32,
}

/// Opens `uri` in the host browser / handler. `entity` is carried for
/// family uniformity; the apply does not read it.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalOpenUri {
    /// The terminal entity the link belongs to (unused by the apply).
    #[event_target]
    pub entity: Entity,
    /// The URI to open.
    pub uri: String,
}

/// Carries a gather system's decided mouse effects to the apply observer, so the
/// dispatch systems stay read-only on the terminal and all mutation lives in one
/// place (`on_terminal_mouse_effects`), mirroring `PasteAction` / `on_paste`.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalMouseEffects {
    /// The terminal entity to apply the effects to.
    #[event_target]
    entity: Entity,
    /// The decided effects, applied in order.
    effects: Vec<MouseEffect>,
}

impl TerminalMouseEffects {
    /// Builds a mouse-effects event targeting `entity` with `effects` applied in order.
    pub fn new(entity: Entity, effects: Vec<MouseEffect>) -> Self {
        Self { entity, effects }
    }

    /// The decided effects this event applies, in order.
    pub fn effects(&self) -> &[MouseEffect] {
        &self.effects
    }
}

/// Registers the crate's mouse-effect apply observer.
pub(crate) struct OzmaMousePlugin;

impl Plugin for OzmaMousePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_mouse_effects)
            .add_observer(on_terminal_mouse_write)
            .add_observer(on_terminal_selection_start)
            .add_observer(on_terminal_selection_update)
            .add_observer(on_terminal_selection_clear)
            .add_observer(on_terminal_selection_copy)
            .add_observer(on_terminal_viewport_scroll)
            .add_observer(on_terminal_open_uri);
    }
}

/// Applies a gather system's decided mouse effects to the target terminal — the
/// sole apply path for the shared mouse dispatch. Runs at command flush (same
/// frame as the trigger), mirroring `on_paste` / `on_terminal_key_input`.
fn on_terminal_mouse_effects(
    ev: On<TerminalMouseEffects>,
    mut commands: Commands,
    mut clipboard: ResMut<Clipboard>,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    if let (Some(mut pty), Some(mut coalescer)) = (pty, coalescer) {
        for effect in &ev.effects {
            apply_effect(
                &mut handle,
                &mut pty,
                &mut coalescer,
                &mut clipboard,
                effect,
            );
        }
        return;
    }
    let mut dirty = false;
    for effect in &ev.effects {
        dirty |= apply_effect_detached(
            &mut handle,
            &mut clipboard,
            &mut commands,
            ev.entity,
            effect,
        );
    }
    if dirty {
        handle.flush_emit(&mut commands, ev.entity);
    }
}

fn apply_effect(
    handle: &mut TerminalHandle,
    pty: &mut PtyHandle,
    coalescer: &mut Coalescer,
    clipboard: &mut Clipboard,
    effect: &MouseEffect,
) {
    match effect {
        MouseEffect::Write(b) => {
            if let Err(e) = handle.write(pty, b) {
                tracing::warn!(?e, "ozma mouse pty write failed");
            }
        }
        MouseEffect::SelStart { point, side, ty } => {
            handle.selection_start_at(coalescer, *point, *side, *ty)
        }
        MouseEffect::SelUpdate { point, side } => {
            handle.selection_update_to(coalescer, *point, *side)
        }
        MouseEffect::SelClear => handle.selection_clear(coalescer),
        MouseEffect::Copy => {
            if let Some(text) = handle.selection_to_string() {
                clipboard.write(text);
            }
        }
        MouseEffect::Scroll(lines) => handle.scroll(coalescer, *lines),
        MouseEffect::OpenUri(uri) => try_open_uri(uri),
    }
}

fn apply_effect_detached(
    handle: &mut TerminalHandle,
    clipboard: &mut Clipboard,
    commands: &mut Commands,
    entity: Entity,
    effect: &MouseEffect,
) -> bool {
    match effect {
        MouseEffect::Write(b) => {
            commands.trigger(TerminalForwardInput {
                entity,
                bytes: b.clone(),
            });
            false
        }
        MouseEffect::SelStart { point, side, ty } => {
            handle.selection_start_at_vt_only(*point, *side, *ty);
            true
        }
        MouseEffect::SelUpdate { point, side } => {
            handle.selection_update_to_vt_only(*point, *side);
            true
        }
        MouseEffect::SelClear => {
            handle.selection_clear_vt_only();
            true
        }
        MouseEffect::Copy => {
            if let Some(text) = handle.selection_to_string() {
                clipboard.write(text);
            }
            false
        }
        MouseEffect::Scroll(lines) => {
            handle.scroll_vt_only(*lines);
            true
        }
        MouseEffect::OpenUri(uri) => {
            try_open_uri(uri);
            false
        }
    }
}

/// Applies one handle-touching mouse op to `entity`, branching on whether
/// the terminal is PTY-attached (apply through the coalescer) or detached
/// (mutate the VT only, then `flush_emit`). `detached` returns whether a
/// frame flush is needed (the write op forwards instead and returns false).
fn apply_to_terminal(
    commands: &mut Commands,
    handle: &mut TerminalHandle,
    pty: Option<Mut<PtyHandle>>,
    coalescer: Option<Mut<Coalescer>>,
    entity: Entity,
    attached: impl FnOnce(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
    detached: impl FnOnce(&mut Commands, &mut TerminalHandle, Entity) -> bool,
) {
    if let (Some(mut pty), Some(mut coalescer)) = (pty, coalescer) {
        attached(handle, &mut pty, &mut coalescer);
    } else if detached(commands, handle, entity) {
        handle.flush_emit(commands, entity);
    }
}

/// Applies a `TerminalMouseWrite`: PTY write when attached, otherwise a
/// `TerminalForwardInput` to the host-owned backend router.
fn on_terminal_mouse_write(
    ev: On<TerminalMouseWrite>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, pty, _coalescer| {
            if let Err(e) = handle.write(pty, &ev.bytes) {
                tracing::warn!(?e, "ozma mouse pty write failed");
            }
        },
        |commands, _handle, entity| {
            commands.trigger(TerminalForwardInput {
                entity,
                bytes: ev.bytes.clone(),
            });
            false
        },
    );
}

/// Applies a `TerminalSelectionStart`.
fn on_terminal_selection_start(
    ev: On<TerminalSelectionStart>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, _pty, coalescer| handle.selection_start_at(coalescer, ev.point, ev.side, ev.ty),
        |_commands, handle, _entity| {
            handle.selection_start_at_vt_only(ev.point, ev.side, ev.ty);
            true
        },
    );
}

/// Applies a `TerminalSelectionUpdate`.
fn on_terminal_selection_update(
    ev: On<TerminalSelectionUpdate>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, _pty, coalescer| handle.selection_update_to(coalescer, ev.point, ev.side),
        |_commands, handle, _entity| {
            handle.selection_update_to_vt_only(ev.point, ev.side);
            true
        },
    );
}

/// Applies a `TerminalSelectionClear`.
fn on_terminal_selection_clear(
    ev: On<TerminalSelectionClear>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, _pty, coalescer| handle.selection_clear(coalescer),
        |_commands, handle, _entity| {
            handle.selection_clear_vt_only();
            true
        },
    );
}

/// Applies a `TerminalViewportScroll`.
fn on_terminal_viewport_scroll(
    ev: On<TerminalViewportScroll>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, _pty, coalescer| handle.scroll(coalescer, ev.lines),
        |_commands, handle, _entity| {
            handle.scroll_vt_only(ev.lines);
            true
        },
    );
}

/// Applies a `TerminalSelectionCopy`: writes the selection text (if any) to
/// the clipboard. Needs only read access to the handle.
fn on_terminal_selection_copy(
    ev: On<TerminalSelectionCopy>,
    mut clipboard: ResMut<Clipboard>,
    terminals: Query<&TerminalHandle, With<OzmaTerminal>>,
) {
    let Ok(handle) = terminals.get(ev.entity) else {
        return;
    };
    if let Some(text) = handle.selection_to_string() {
        clipboard.write(text);
    }
}

/// Applies a `TerminalOpenUri`: opens the link in the host handler.
fn on_terminal_open_uri(ev: On<TerminalOpenUri>) {
    try_open_uri(&ev.uri);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_tty_engine::{Column, Line};

    #[test]
    fn detached_terminal_forwards_write_and_selects_via_vt_only() {
        use ozma_tty_engine::TerminalHandle;

        #[derive(Resource, Default)]
        struct CapturedForward(Vec<Vec<u8>>);

        let mut app = App::new();
        app.init_resource::<Clipboard>()
            .init_resource::<CapturedForward>()
            .add_observer(on_terminal_mouse_effects)
            .add_observer(
                |ev: On<TerminalForwardInput>, mut cap: ResMut<CapturedForward>| {
                    cap.0.push(ev.bytes.clone());
                },
            );

        let handle = TerminalHandle::detached(10, 5);
        let entity = app.world_mut().spawn((OzmaTerminal, handle)).id();

        app.world_mut().trigger(TerminalMouseEffects {
            entity,
            effects: vec![MouseEffect::Write(b"\x1b[<0;1;1M".to_vec())],
        });
        app.world_mut().trigger(TerminalMouseEffects {
            entity,
            effects: vec![MouseEffect::SelStart {
                point: Point::new(Line(0), Column(0)),
                side: Side::Left,
                ty: SelectionType::Simple,
            }],
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<CapturedForward>().0,
            vec![b"\x1b[<0;1;1M".to_vec()],
            "Write on a PTY-less OzmaTerminal must emit TerminalForwardInput"
        );
        let handle = app.world().entity(entity).get::<TerminalHandle>().unwrap();
        assert!(
            handle.selection_to_string().is_some(),
            "SelStart on a PTY-less OzmaTerminal must set a selection via vt_only"
        );
    }

    #[test]
    fn mouse_effects_on_entity_without_terminal_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Clipboard>()
            .add_observer(on_terminal_mouse_effects);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(TerminalMouseEffects {
            entity,
            effects: vec![MouseEffect::Scroll(3)],
        });
        app.update();
        // Reaching here proves the observer handles the missing-terminal path
        // without panicking; effect correctness is covered by the decide_* tests.
    }

    #[test]
    fn detached_write_event_forwards_bytes() {
        use ozma_tty_engine::TerminalHandle;

        #[derive(Resource, Default)]
        struct CapturedForward(Vec<Vec<u8>>);

        let mut app = App::new();
        app.init_resource::<Clipboard>()
            .init_resource::<CapturedForward>()
            .add_observer(on_terminal_mouse_write)
            .add_observer(
                |ev: On<TerminalForwardInput>, mut cap: ResMut<CapturedForward>| {
                    cap.0.push(ev.bytes.clone());
                },
            );

        let handle = TerminalHandle::detached(10, 5);
        let entity = app.world_mut().spawn((OzmaTerminal, handle)).id();

        app.world_mut().trigger(TerminalMouseWrite {
            entity,
            bytes: b"\x1b[<0;1;1M".to_vec(),
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<CapturedForward>().0,
            vec![b"\x1b[<0;1;1M".to_vec()],
            "TerminalMouseWrite on a PTY-less OzmaTerminal must emit TerminalForwardInput"
        );
    }

    #[test]
    fn detached_selection_start_event_sets_selection_via_vt_only() {
        use ozma_tty_engine::TerminalHandle;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Clipboard>()
            .add_observer(on_terminal_selection_start);

        let handle = TerminalHandle::detached(10, 5);
        let entity = app.world_mut().spawn((OzmaTerminal, handle)).id();

        app.world_mut().trigger(TerminalSelectionStart {
            entity,
            point: Point::new(Line(0), Column(0)),
            side: Side::Left,
            ty: SelectionType::Simple,
        });
        app.update();

        let handle = app.world().entity(entity).get::<TerminalHandle>().unwrap();
        assert!(
            handle.selection_to_string().is_some(),
            "TerminalSelectionStart on a PTY-less OzmaTerminal must set a selection via vt_only"
        );
    }

    #[test]
    fn viewport_scroll_event_on_missing_terminal_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Clipboard>()
            .add_observer(on_terminal_viewport_scroll);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .trigger(TerminalViewportScroll { entity, lines: 3 });
        app.update();
    }
}
