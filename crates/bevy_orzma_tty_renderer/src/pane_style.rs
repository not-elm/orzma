//! The dimming and tinting a host applies to a terminal pane that is not
//! the active pane.

use bevy::prelude::{Component, Vec4};

/// Per-pane inactive-pane treatment for the terminal renderer: a background
/// `tint` (rgb = target color in LINEAR space, `a` = blend amount) and a
/// brightness `dim`. The shader blends each background source toward `tint.rgb`
/// by `tint.a` before glyphs/overlays paint (background only), then multiplies
/// the final color by `dim`. An absent component is treated as
/// `{ dim: 1.0, tint: ZERO }` (full-bright, untinted / active).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct PaneInactiveStyle {
    /// Brightness multiplier in `0.0..=1.0`; `1.0` = full-bright.
    pub dim: f32,
    /// Background tint: rgb = target color (linear), `a` = blend amount in
    /// `0.0..=1.0` (`0.0` = no tint / active).
    pub tint: Vec4,
    /// Inline-overlay (webview) brightness multiplier in `0.0..=1.0`; `1.0` =
    /// full-bright. Applied to overlay samples only, independent of `tint`.
    pub overlay_dim: f32,
    /// Inline-overlay (webview) desaturation in `0.0..=1.0`; `0.0` = full color,
    /// `1.0` = grey.
    pub overlay_desaturate: f32,
}

impl Default for PaneInactiveStyle {
    fn default() -> Self {
        Self {
            dim: 1.0,
            tint: Vec4::ZERO,
            overlay_dim: 1.0,
            overlay_desaturate: 0.0,
        }
    }
}
