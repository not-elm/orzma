//! Apply observer for tmux mouse effects.
//!
//! Receives `TmuxMouseEffects` triggered by `tmux_gesture` and applies them:
//! `SelectPane` / `ResizePane` send tmux control-mode commands, gated on an
//! active `TmuxClient`; the copy-drag variants trigger local
//! `TerminalSelection*` events on the pane's own terminal handle directly, with
//! no `TmuxClient` dependency. Bookkeeping on `TmuxMouseGesture` is done here
//! exactly when a send/trigger succeeds, preserving invariant 8.

use super::TmuxMouseGesture;
use super::effect::{MultiSelectKind, TmuxMouseEffect, TmuxMouseEffects};
use crate::action::terminal::{
    TerminalSelectionCopy, TerminalSelectionStart, TerminalSelectionUpdate,
};
use bevy::prelude::*;
use orzma_tmux::{ResizePaneX, ResizePaneY, SelectPane, TmuxClient};
use orzma_tty_engine::SelectionType;
use tmux_control_parser::DividerAxis;

/// Plugin that registers the tmux mouse-effects apply observer.
pub(super) struct ApplyPlugin;

impl Plugin for ApplyPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_tmux_mouse_effects);
    }
}

/// Observer that applies a frame's decided `TmuxMouseEffects`. `SelectPane` /
/// `ResizePane` sends are gated on an active client (a no-client frame is a
/// no-op for those two variants); the copy-drag variants trigger local
/// `TerminalSelection*` events unconditionally.
fn on_tmux_mouse_effects(
    ev: On<TmuxMouseEffects>,
    mut commands: Commands,
    mut gesture: ResMut<TmuxMouseGesture>,
    mut client: Option<Single<&mut TmuxClient>>,
) {
    for effect in &ev.effects {
        match *effect {
            TmuxMouseEffect::SelectPane(pane_id) => {
                let Some(handle) = client.as_deref_mut() else {
                    continue;
                };
                if let Err(e) = handle.send(SelectPane { id: pane_id }) {
                    tracing::warn!(?e, pane = pane_id.0, "select-pane send failed");
                }
            }
            TmuxMouseEffect::ResizePane {
                axis,
                primary,
                size,
            } => {
                let Some(handle) = client.as_deref_mut() else {
                    continue;
                };
                let result = match axis {
                    DividerAxis::Vertical => handle.send(ResizePaneX {
                        id: primary,
                        width: size,
                    }),
                    DividerAxis::Horizontal => handle.send(ResizePaneY {
                        id: primary,
                        height: size,
                    }),
                };
                if let Err(e) = result {
                    tracing::warn!(?e, pane = primary.0, "resize-pane send failed");
                    continue;
                }
                if let super::GestureState::Resizing {
                    last_sent, resized, ..
                } = &mut gesture.state
                {
                    *last_sent = size;
                    *resized = true;
                }
            }
            TmuxMouseEffect::BeginCopyDrag {
                entity,
                anchor,
                side,
                ty,
            } => {
                commands.trigger(TerminalSelectionStart {
                    entity,
                    point: anchor,
                    side,
                    ty,
                });
                if let super::GestureState::Selecting {
                    begun, last_target, ..
                } = &mut gesture.state
                {
                    *begun = true;
                    *last_target = Some(anchor);
                }
            }
            TmuxMouseEffect::ExtendCopyDrag { entity, cell, side } => {
                commands.trigger(TerminalSelectionUpdate {
                    entity,
                    point: cell,
                    side,
                });
                if let super::GestureState::Selecting { last_target, .. } = &mut gesture.state {
                    *last_target = Some(cell);
                }
            }
            TmuxMouseEffect::MultiSelect {
                entity,
                kind,
                cell,
                side,
            } => {
                let ty = match kind {
                    MultiSelectKind::Word => SelectionType::Semantic,
                    MultiSelectKind::Line => SelectionType::Lines,
                };
                commands.trigger(TerminalSelectionStart {
                    entity,
                    point: cell,
                    side,
                    ty,
                });
                commands.trigger(TerminalSelectionCopy { entity });
            }
            TmuxMouseEffect::CopySelection { entity } => {
                commands.trigger(TerminalSelectionCopy { entity });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_tmux::PaneId;

    #[test]
    fn observer_applies_without_panic_when_no_client() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<TmuxMouseGesture>()
            .add_observer(on_tmux_mouse_effects);
        let e = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(TmuxMouseEffects {
            entity: e,
            effects: vec![
                TmuxMouseEffect::SelectPane(PaneId(1)),
                TmuxMouseEffect::CopySelection { entity: e },
            ],
        });
        app.world_mut().flush();
    }
}
