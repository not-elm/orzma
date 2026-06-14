use serde::{Deserialize, Serialize};

/// Bit 0 of the packed `cursor_style` u32 — set when the cursor
/// should be drawn. The WGSL shader short-circuits when this bit is
/// clear (see `terminal_ui_material.wgsl:74`). Exposed so app-level
/// overrides (e.g., `TerminalGrid.suppress_cursor`) can mask it out
/// without re-deriving the literal `1`.
pub const CURSOR_VISIBLE_BIT: u32 = 1;

/// Vi-mode cursor position in viewport coordinates.
///
/// When the user is in alacritty vi mode (= tmux copy mode), the server
/// always tries to keep the cursor inside the visible viewport via
/// `Term::scroll_display`. `in_scrollback` is the safety valve: when
/// `true`, the cursor sits above the viewport, the client should skip
/// rendering, and `row` is clamped to `-1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViCursor {
    /// Viewport row; `-1` when `in_scrollback` is true.
    pub row: i16,
    /// Viewport column (0-based).
    pub column: u16,
    /// True when the vi cursor is above the viewport (in scrollback).
    pub in_scrollback: bool,
}

/// Cursor state at snapshot time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// Column position (0-based).
    pub x: u16,
    /// Row position (0-based).
    pub y: u16,
    /// Visual shape selected by DECSCUSR.
    pub shape: CursorShape,
    /// True when DECSCUSR selects a blinking variant. Steady variants
    /// (`\033[2 q`, `\033[4 q`, `\033[6 q`) set this to false.
    pub blinking: bool,
    /// True when the cursor should be rendered. Gated by DECTCEM
    /// (`TermMode::SHOW_CURSOR`) AND DECSCUSR shape != Hidden.
    pub visible: bool,
}

impl Cursor {
    pub fn pack_cursor_style(&self) -> u32 {
        let visible = if self.visible { CURSOR_VISIBLE_BIT } else { 0 };
        let shape = match self.shape {
            CursorShape::Block => 0u32,
            CursorShape::Underline => 1,
            CursorShape::Bar => 2,
        };
        let blinking = if self.blinking { 1u32 } else { 0 };
        visible | (shape << 1) | (blinking << 3)
    }
}

/// Terminal cursor shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    /// Block cursor.
    #[default]
    Block,
    /// Underline cursor.
    Underline,
    /// Bar (vertical line) cursor.
    Bar,
}
