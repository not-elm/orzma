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
/// Active tab background — slightly lighter than the tab bar.
pub const TAB_ACTIVE_BG: Color = Color::srgb(0.145, 0.157, 0.188);
/// Pane border — cool blue-grey, intentionally subtle.
pub const BORDER: Color = Color::srgb(0.333, 0.333, 0.333);
/// Primary text color.
pub const FOREGROUND: Color = Color::srgb(0.870, 0.870, 0.890);
/// Secondary / muted text color (inactive tab text).
pub const MUTED: Color = Color::srgb(0.376, 0.408, 0.471);
/// Active highlight (active session chip in status bar, active-pane top
/// indicator on the active tab).
pub const ACCENT: Color = Color::srgb(0.302, 0.561, 0.851);

/// `ActivityKind::Terminal` placeholder background.
pub const ACTIVITY_TERMINAL: Color = Color::srgb(0.094, 0.094, 0.110);
/// `ActivityKind::Browser` placeholder background.
pub const ACTIVITY_BROWSER: Color = Color::srgb(0.094, 0.149, 0.220);
/// `ActivityKind::Extension` placeholder background.
pub const ACTIVITY_EXTENSION: Color = Color::srgb(0.094, 0.180, 0.110);

/// Pane border thickness.
pub const PANE_BORDER_PX: f32 = 1.0;
/// Generic padding inside elements (status bar segments).
pub const ELEMENT_PADDING_PX: f32 = 6.0;
/// Horizontal padding inside a single tab.
pub const TAB_PADDING_X_PX: f32 = 8.0;
/// Top-corner radius applied to all pane tabs.
pub const TAB_BORDER_RADIUS_PX: f32 = 4.0;
/// Top indicator thickness on the active pane tab.
pub const TAB_INDICATOR_PX: f32 = 2.0;
pub const BORDER_PX: f32 = 1.0;

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
/// Horizontal padding inside the copy-mode indicator chip. Tighter than
/// `ELEMENT_PADDING_PX` because the chip's text is also smaller.
pub const COPY_MODE_INDICATOR_PADDING_X_PX: f32 = 4.0;

pub const UI_FONT_SIZE: f32 = 12.0;
