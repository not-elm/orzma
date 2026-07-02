//! Shared VI (copy-mode) action events: one `EntityEvent` per operation kind,
//! fired by both modes' copy-mode key gathers and applied by mode-specific
//! observers (`vi/default_mode.rs` locally, `vi/tmux_mode.rs` via tmux
//! `send-keys -X`).

mod default_mode;
mod keymap;
mod tmux_mode;

use bevy::prelude::*;
pub(crate) use keymap::{ResolvedCopyModeKeys, trigger_copy_mode_action};
use ozma_tty_engine::{SelectionType, ViMotion};
use ozmux_tmux::PromptKind;

/// A viewport scroll kind, shared by both appliers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViScrollKind {
    /// One page toward history.
    PageUp,
    /// One page toward the tail.
    PageDown,
    /// Half a page toward history.
    HalfUp,
    /// Half a page toward the tail.
    HalfDown,
    /// One line toward history.
    LineUp,
    /// One line toward the tail.
    LineDown,
    /// Oldest history line.
    Top,
    /// The live tail.
    Bottom,
}

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
    pub kind: ViScrollKind,
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

/// Opens a search / jump prompt for `entity` (tmux mode only for now).
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViPromptRequest {
    /// The copy-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// Which prompt to open.
    pub kind: PromptKind,
}

/// Repeats the previous search on `entity` (tmux mode only for now).
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViSearchStepRequest {
    /// The copy-mode surface entity.
    #[event_target]
    pub entity: Entity,
    /// `true` repeats in the original direction (`n`), `false` reversed (`N`).
    pub forward: bool,
}

/// Aggregates the per-mode VI appliers.
pub(crate) struct ViActionPlugin;

impl Plugin for ViActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            keymap::CopyModeKeymapPlugin,
            default_mode::DefaultModeViPlugin,
            tmux_mode::TmuxModeViPlugin,
        ));
    }
}
