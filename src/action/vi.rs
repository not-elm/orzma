//! Shared VI (vi-mode) action events: one `EntityEvent` per operation kind,
//! fired by the vi-mode key gather and applied by `vi/applier.rs`'s
//! local terminal-engine observers, for every pane, tmux and non-tmux alike.

mod applier;
mod keymap;

use bevy::prelude::*;
pub(crate) use keymap::{ResolvedViModeKeys, trigger_vi_mode_action};
use orzma_configs::vi_mode::ViModeScroll;
use orzma_tmux::PromptKind;
use orzma_tty_engine::{SelectionType, ViMotion};

/// Moves the copy cursor on `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViMotionRequest {
    /// The vi-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// The motion to apply.
    pub motion: ViMotion,
}

/// Scrolls `entity`'s vi-mode viewport.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViScrollRequest {
    /// The vi-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// The scroll to apply.
    pub kind: ViModeScroll,
}

/// Toggles a selection of kind `ty` on `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViSelectionToggleRequest {
    /// The vi-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// The selection kind to toggle.
    pub ty: SelectionType,
}

/// Copies `entity`'s selection to the clipboard and leaves copy mode.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViYankRequest {
    /// The vi-mode surface entity.
    #[event_target]
    pub entity: Entity,
}

/// Leaves copy mode on `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViExitRequest {
    /// The vi-mode surface entity.
    #[event_target]
    pub entity: Entity,
}

/// Opens a search / jump prompt for `entity`. Nothing constructs this event
/// in v1 — `trigger_vi_mode_action`'s `Prompt` arm is a deliberate no-op
/// until local vi-mode search ships, and the tmux `send-keys -X` applier
/// that used to consume it was removed. Kept so the event/type surface is
/// ready for that follow-up PR.
#[derive(EntityEvent, Debug, Clone)]
#[expect(
    dead_code,
    reason = "no constructor until local vi-mode search wires trigger_vi_mode_action's Prompt arm back up"
)]
pub(crate) struct ViPromptRequest {
    /// The vi-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// Which prompt to open.
    pub kind: PromptKind,
}

/// Repeats the previous search on `entity`. No constructor yet — see
/// `ViPromptRequest`'s doc comment.
#[derive(EntityEvent, Debug, Clone)]
#[expect(
    dead_code,
    reason = "no constructor until local vi-mode search wires trigger_vi_mode_action's SearchStep arm back up"
)]
pub(crate) struct ViSearchStepRequest {
    /// The vi-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// `true` repeats in the original direction (`n`), `false` reversed (`N`).
    pub forward: bool,
}

/// Wires the vi-mode keymap to the local VI applier.
pub(crate) struct ViActionPlugin;

impl Plugin for ViActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((keymap::ViModeKeymapPlugin, applier::ViApplierPlugin));
    }
}
