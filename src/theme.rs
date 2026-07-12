//! Hardcoded palette + size constants for the Bevy UI.

use bevy::prelude::Color;

/// Background color of the vi-mode indicator chip. tmux-style bright
/// yellow so the chip reads as a deliberate HUD element on top of the
/// terminal grid.
pub const VI_MODE_INDICATOR_BG: Color = Color::srgb(0.95, 0.85, 0.20);
/// Foreground (text) color of the vi-mode indicator chip. Near-black
/// for contrast against `VI_MODE_INDICATOR_BG`.
pub const VI_MODE_INDICATOR_FG: Color = Color::srgb(0.10, 0.10, 0.10);
/// Font size of the vi-mode indicator chip's text. Smaller than Bevy's
/// 20px default so the chip reads as a compact HUD label instead of
/// competing with the terminal grid.
pub const VI_MODE_INDICATOR_FONT_SIZE_PX: f32 = 11.0;
/// Horizontal padding inside the vi-mode indicator chip. Kept tight
/// because the chip's text is also smaller than the surrounding UI.
pub const VI_MODE_INDICATOR_PADDING_X_PX: f32 = 4.0;
