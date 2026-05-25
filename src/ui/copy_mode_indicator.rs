//! tmux-style copy-mode indicator chip. A `Display::None` chip Node is
//! attached as a child of each Activity host the first frame
//! `TerminalHandle` is observed there; it becomes visible while the
//! host carries `CopyModeState` and shows `[offset/total]` over the
//! pane's top-right corner.

use crate::theme;
use crate::ui::palette;
use bevy::app::{App, Plugin};
use bevy::ecs::component::Component;
use bevy::prelude::*;

/// Marker for the chip Node child of an Activity host. Exactly one
/// per host; created on `Added<TerminalHandle>` and never despawned
/// (visibility toggled via `Node.display`).
#[derive(Component)]
pub(crate) struct CopyModeIndicator;

/// Last `(offset, total)` pair this chip rendered. Compared numerically
/// each frame so `format!` only runs when the pair changed.
#[derive(Component, Default, Debug, PartialEq, Eq)]
pub(crate) struct IndicatorCache {
    pub offset: u32,
    pub total: u32,
}

/// Formats the chip body as `[offset/total]` — tmux compatible.
pub(crate) fn format_indicator(offset: u32, total: u32) -> String {
    format!("[{offset}/{total}]")
}

/// Bevy Plugin: wires the copy-mode indicator's attach + refresh systems
/// and the exit observer.
pub struct CopyModeIndicatorPlugin;

impl Plugin for CopyModeIndicatorPlugin {
    fn build(&self, _app: &mut App) {
        // TODO: register attach + refresh systems (Task 4 / Task 5) and the
        // On<Remove, CopyModeState> observer (Task 7).
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_indicator_matches_tmux_default() {
        assert_eq!(format_indicator(0, 429), "[0/429]");
        assert_eq!(format_indicator(3, 429), "[3/429]");
        assert_eq!(format_indicator(0, 0), "[0/0]");
    }
}
