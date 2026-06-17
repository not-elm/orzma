//! Hardcoded palette + size constants for the Bevy UI. Token loading from
//! the `ozmux_configs` theme module is deferred to a later phase.
//!
//! Values are tuned to approximate cmux/tmux-style chrome: cool blue-grey
//! pane content, distinctly darker tab bar, very subtle pane dividers.

use bevy::prelude::Color;

/// Pane content background — cool blue-grey, matches cmux's terminal area.
pub const BACKGROUND: Color = Color::srgb(0.118, 0.125, 0.157);
/// Status bar background — slightly darker than BACKGROUND to read as
/// secondary chrome below the active pane content.
pub const PANEL: Color = Color::srgb(0.110, 0.118, 0.157);
/// Tab bar background — distinctly darker than pane content so chrome
/// reads separate from content.
pub const TAB_BAR_BG: Color = Color::srgb(0.086, 0.094, 0.125);
/// Pane border — cool blue-grey, intentionally subtle.
pub const BORDER: Color = Color::srgb(0.333, 0.333, 0.333);
/// Primary text color.
pub const FOREGROUND: Color = Color::srgb(0.870, 0.870, 0.890);
/// Secondary / muted text color (inactive tab text).
pub const MUTED: Color = Color::srgb(0.376, 0.408, 0.471);
/// Active highlight (active workspace chip in status bar, active-pane top
/// indicator on the active tab).
pub const ACCENT: Color = Color::srgb(0.302, 0.561, 0.851);
/// Session-block background in the window bar — neutral slate. Distinct from
/// `PANEL` so the trailing powerline arrow reads, and from `ACCENT` so accent
/// uniquely marks the active window.
pub const SESSION_BG: Color = Color::srgb(0.200, 0.220, 0.282);
/// Window-flag warning color (bell `!` / activity `#`) — amber.
pub const FLAG_WARN: Color = Color::srgb(0.878, 0.690, 0.302);
/// Powerline right-pointing filled separator glyph (Nerd Font U+E0B0).
pub const POWERLINE_RIGHT: &str = "\u{e0b0}";
/// Powerline left-pointing filled separator glyph (Nerd Font U+E0B2).
pub const POWERLINE_LEFT: &str = "\u{e0b2}";
/// Session-chooser selection bar — tmux choose-tree style amber.
pub const SELECTION: Color = Color::srgb(0.847, 0.651, 0.341);
/// Text on the SELECTION bar — near-black for contrast.
pub const SELECTION_FG: Color = Color::srgb(0.094, 0.086, 0.063);
/// Faint divider line (chooser footer separator, etc.).
pub const DIVIDER: Color = Color::srgba(1.0, 1.0, 1.0, 0.06);
/// Session-chooser title / footer font size.
pub const PICKER_TITLE_FONT_SIZE_PX: f32 = 11.0;

/// Pane border thickness.
pub const PANE_BORDER_PX: f32 = 1.0;
/// Gap in logical px between packed panes. The grey window container bleeds
/// through this gap as the 1px divider line.
pub const PANE_GAP_PX: f32 = 1.0;
/// Camera clear color — shows through tmux's reserved-cell gaps between panes.
/// Black matches the terminal background; retint here to recolor the gaps.
pub const PANE_GAP: Color = Color::BLACK;
/// Horizontal padding inside a single tab.
pub const TAB_PADDING_X_PX: f32 = 8.0;

/// Background color of the copy-mode indicator chip. tmux-style bright
/// yellow so the chip reads as a deliberate HUD element on top of the
/// terminal grid.
pub const COPY_MODE_INDICATOR_BG: Color = Color::srgb(0.95, 0.85, 0.20);
/// Foreground (text) color of the copy-mode indicator chip. Near-black
/// for contrast against `COPY_MODE_INDICATOR_BG`.
pub const COPY_MODE_INDICATOR_FG: Color = Color::srgb(0.10, 0.10, 0.10);
/// Font size of the copy-mode indicator chip's text. Smaller than Bevy's
/// 20px default so the chip reads as a compact HUD label instead of
/// competing with the terminal grid.
pub const COPY_MODE_INDICATOR_FONT_SIZE_PX: f32 = 11.0;
/// Horizontal padding inside the copy-mode indicator chip. Kept tight
/// because the chip's text is also smaller than the surrounding UI.
pub const COPY_MODE_INDICATOR_PADDING_X_PX: f32 = 4.0;

pub const UI_FONT_SIZE: f32 = 12.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powerline_right_is_the_nerd_font_code_point() {
        assert_eq!(POWERLINE_RIGHT, "\u{e0b0}");
    }
}
