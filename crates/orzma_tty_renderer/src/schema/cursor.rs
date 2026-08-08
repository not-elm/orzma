use serde::{Deserialize, Serialize};

/// Bit 0 of the packed `cursor_style` u32 — set when the cursor
/// should be drawn. The WGSL shader short-circuits when this bit is
/// clear (see `terminal_ui_material.wgsl:74`). Exposed so app-level
/// overrides (e.g., `TerminalGrid.suppress_cursor`) can mask it out
/// without re-deriving the literal `1`.
pub const CURSOR_VISIBLE_BIT: u32 = 1;
