//! Shared VI (copy-mode) action events: one `EntityEvent` per operation kind,
//! fired by the copy-mode key gather and applied by `vi/applier.rs`'s
//! local terminal-engine observers, for every pane, tmux and non-tmux alike.

mod applier;
mod keymap;

use bevy::prelude::*;
pub(crate) use keymap::{ResolvedCopyModeKeys, trigger_copy_mode_action};
use ozma_tty_engine::{SelectionType, ViMotion};
use ozmux_configs::copy_mode::CopyScroll;
use ozmux_tmux::PromptKind;

/// Moves the copy cursor on `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViMotionRequest {
    /// The copy-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// The motion to apply.
    pub motion: ViMotion,
}

/// Scrolls `entity`'s copy-mode viewport.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViScrollRequest {
    /// The copy-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// The scroll to apply.
    pub kind: CopyScroll,
}

/// Toggles a selection of kind `ty` on `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViSelectionToggleRequest {
    /// The copy-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// The selection kind to toggle.
    pub ty: SelectionType,
}

/// Copies `entity`'s selection to the clipboard and leaves copy mode.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViYankRequest {
    /// The copy-mode surface entity.
    #[event_target]
    pub entity: Entity,
}

/// Leaves copy mode on `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViExitRequest {
    /// The copy-mode surface entity.
    #[event_target]
    pub entity: Entity,
}

/// Opens a search / jump prompt for `entity`. Nothing constructs this event
/// in v1 — `trigger_copy_mode_action`'s `Prompt` arm is a deliberate no-op
/// until local copy-mode search ships, and the tmux `send-keys -X` applier
/// that used to consume it was removed. Kept so the event/type surface is
/// ready for that follow-up PR.
#[derive(EntityEvent, Debug, Clone)]
#[expect(
    dead_code,
    reason = "no constructor until local copy-mode search wires trigger_copy_mode_action's Prompt arm back up"
)]
pub(crate) struct ViPromptRequest {
    /// The copy-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// Which prompt to open.
    #[expect(
        dead_code,
        reason = "no applier reads this until prompt/search-step support lands"
    )]
    pub kind: PromptKind,
}

/// Repeats the previous search on `entity`. No constructor yet — see
/// `ViPromptRequest`'s doc comment.
#[derive(EntityEvent, Debug, Clone)]
#[expect(
    dead_code,
    reason = "no constructor until local copy-mode search wires trigger_copy_mode_action's SearchStep arm back up"
)]
pub(crate) struct ViSearchStepRequest {
    /// The copy-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// `true` repeats in the original direction (`n`), `false` reversed (`N`).
    #[expect(
        dead_code,
        reason = "no applier reads this until prompt/search-step support lands"
    )]
    pub forward: bool,
}

/// Wires the copy-mode keymap to the local VI applier.
pub(crate) struct ViActionPlugin;

impl Plugin for ViActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((keymap::CopyModeKeymapPlugin, applier::ViApplierPlugin));
    }
}
