//! The inline-overlay (webview) slots a terminal's material composites
//! over its cells.

use bevy::prelude::{Component, Handle, IVec4, Image};

/// Number of inline-overlay texture slots on `TerminalUiMaterial`.
///
/// Slot index = array index into `overlays` / `overlay_rects`; the WGSL
/// texture binding is `OVERLAY_TEX_BINDING_BASE + i`. Hard upper bound per
/// terminal surface.
pub const OVERLAY_SLOTS: usize = 12;

/// Per-terminal overlay placements and textures.
///
/// `rects[i]` is `(row, col, rows, cols)` in CELL coordinates; `row` may be
/// negative when the rect starts above the viewport. `rows == 0` is the
/// inactive-slot sentinel: the shader skips the slot, while the renderer
/// binds `textures[i]` regardless, so consumers should set freed slots to
/// `None`.
///
/// Consumers must rebuild this component from live state every frame
/// (all-sentinel start), so stale texture handles cannot outlive their
/// producers.
#[derive(Component, Clone, Debug)]
pub struct TerminalOverlays {
    /// Placement rects, slot-indexed: `(row, col, rows, cols)` in cells.
    pub rects: [IVec4; OVERLAY_SLOTS],
    /// Texture handles, slot-indexed; `None` for inactive slots.
    pub textures: [Option<Handle<Image>>; OVERLAY_SLOTS],
}

impl Default for TerminalOverlays {
    fn default() -> Self {
        Self {
            rects: [IVec4::ZERO; OVERLAY_SLOTS],
            textures: [const { None }; OVERLAY_SLOTS],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_overlays_default_is_all_sentinel() {
        let o = TerminalOverlays::default();
        assert!(
            o.rects.iter().all(|r| r.z == 0),
            "rows == 0 sentinel on every slot"
        );
        assert!(o.textures.iter().all(Option::is_none));
    }
}
