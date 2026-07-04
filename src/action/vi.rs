//! Shared VI (copy-mode) action events: one `EntityEvent` per operation kind,
//! fired by the copy-mode key gather and applied by `vi/default_mode.rs`'s
//! local terminal-engine observers, for every pane, tmux and non-tmux alike.

mod default_mode;
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

/// Opens a search / jump prompt for `entity`. No applier reads this yet — the
/// tmux `send-keys -X` applier that used to handle it was removed and the
/// local applier has no prompt/search-step support (ignored by design, see
/// `default_mode`'s doc comment).
#[derive(EntityEvent, Debug, Clone)]
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

/// Repeats the previous search on `entity`. No applier reads this yet — see
/// `ViPromptRequest`'s doc comment.
#[derive(EntityEvent, Debug, Clone)]
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

/// Aggregates the per-mode VI appliers.
pub(crate) struct ViActionPlugin;

impl Plugin for ViActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            keymap::CopyModeKeymapPlugin,
            default_mode::DefaultModeViPlugin,
        ));
    }
}
