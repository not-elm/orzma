//! Mouse-effect apply path for the Ozma terminal: the shared
//! `TerminalMouseEffects` / `TerminalForwardInput` events, the `MouseEffect`
//! intent type, and the apply observer (`on_terminal_mouse_effects`) that writes
//! the decided effects to the `TerminalHandle` / `Clipboard` (or forwards them to
//! a PTY-less backend). The mode-neutral mouse dispatch that DECIDES these
//! effects lives in the host (`crate::input::mouse` in the binary);
//! `OzmaTerminalMouseSet` is kept here as the ordering anchor the host schedules
//! its `MouseDisabled` gate maintainer `.before(...)` and its dispatch
//! `.in_set(...)` against.

use crate::clipboard::Clipboard;
use crate::hyperlink::try_open_uri;
use crate::spawn::OzmaTerminal;
use bevy::prelude::*;
use ozma_tty_engine::{Coalescer, Point, PtyHandle, SelectionType, Side, TerminalHandle};

/// Ordering anchor for the host's mouse dispatch. The crate no longer owns any
/// mouse systems; the host registers its dispatch `.in_set(OzmaTerminalMouseSet)`
/// and schedules its `MouseDisabled` gate maintainer `.before(OzmaTerminalMouseSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct OzmaTerminalMouseSet;

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
        app.add_observer(on_terminal_mouse_effects);
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
}
