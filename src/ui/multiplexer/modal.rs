//! Shared "does a prompt own the keyboard" predicate for the confirm and
//! rename prompts.
//!
//! Used two ways: by `confirm_prompt` / `rename_prompt` themselves, to
//! refuse opening a second modal while one is already up; and by the input
//! pipeline (`apply_type`, `resolve_key_effects`, `read_ime_events`) to
//! withhold typing, shortcuts, paste, and IME commits from the focused pane
//! while a prompt is open.

use crate::ui::multiplexer::confirm_prompt::ConfirmState;
use crate::ui::multiplexer::rename_prompt::RenameState;
use bevy::prelude::*;

/// Whether a confirm or rename prompt currently owns the keyboard.
pub(crate) fn any_modal_open(
    confirm: Option<Res<ConfirmState>>,
    rename: Option<Res<RenameState>>,
) -> bool {
    confirm.is_some() || rename.is_some()
}
