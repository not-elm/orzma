//! Shared vi-mode action events: one `EntityEvent` per operation kind,
//! fired by the vi-mode key gather and applied by `vi/applier.rs`'s
//! local terminal-engine observers, for every pane, tmux and non-tmux alike.

mod applier;
mod keymap;

use bevy::prelude::*;
pub(crate) use keymap::{ResolvedViModeKeys, trigger_vi_mode_action};
use orzma_configs::vi_mode::ViModeScroll;
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

/// Copies `entity`'s selection to the clipboard and leaves vi mode.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViYankRequest {
    /// The vi-mode surface entity.
    #[event_target]
    pub entity: Entity,
}

/// Leaves vi mode on `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ViExitRequest {
    /// The vi-mode surface entity.
    #[event_target]
    pub entity: Entity,
}

/// Wires the vi-mode keymap to the local VI applier.
pub(crate) struct ViActionPlugin;

impl Plugin for ViActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((keymap::ViModeKeymapPlugin, applier::ViApplierPlugin));
    }
}
