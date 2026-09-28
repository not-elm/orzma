//! Global hover state for the link under the pointer: an OSC 8 hyperlink
//! or a URL detected in plain text.

use bevy::ecs::entity::Entity;
use bevy::ecs::resource::Resource;
use orzma_vt::prelude::{DetectedUrl, HyperlinkId, ViewportCell};

/// Pointer hover state that drives the hyperlink underline accent.
/// Exactly one cell can be hovered at a time across all panes.
#[derive(Resource, Default, Debug, Clone)]
pub struct HyperlinkHoverState {
    /// Surface-host entity the cursor is over, or `None` when the cursor
    /// is outside every pane.
    pub entity: Option<Entity>,
    /// Hovered wire id, or `None` when the cursor is over an unlinked
    /// cell; meaningful only when `entity` is `Some`.
    pub hyperlink_id: Option<HyperlinkId>,
    /// The cells of the URL detected in the plain text under the pointer;
    /// `None` over an OSC 8 link, over text that shows no URL, or while
    /// the activation modifier is up. Meaningful only when `entity` is
    /// `Some`.
    pub detected: Option<DetectedSpan>,
    /// Whether the activation modifier (Cmd on macOS, Ctrl elsewhere) is
    /// held. It drives the shader's `hover_active` uniform.
    pub modifier_held: bool,
}

/// The cells of a detected URL, first to last in reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedSpan {
    /// The cell showing the URL's first character.
    pub first: ViewportCell,
    /// The cell showing the URL's last character.
    pub last: ViewportCell,
}

impl DetectedSpan {
    /// The first and last cells as indices into a row-major grid `cols`
    /// cells wide.
    pub fn linear(self, cols: u16) -> (u32, u32) {
        let index =
            |cell: ViewportCell| u32::from(cell.row) * u32::from(cols) + u32::from(cell.col);
        (index(self.first), index(self.last))
    }
}

impl From<&DetectedUrl> for DetectedSpan {
    fn from(url: &DetectedUrl) -> Self {
        Self {
            first: url.first,
            last: url.last,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(row: u16, col: u16) -> ViewportCell {
        ViewportCell { row, col }
    }

    /// Asserts that a span's cells map to row-major indices of a grid of
    /// the given width.
    ///
    /// Case: a detected URL starts near the end of one row of an
    /// 80-column pane and ends on the next row.
    #[test]
    fn a_span_maps_to_row_major_indices() {
        let span = DetectedSpan {
            first: cell(0, 70),
            last: cell(1, 12),
        };
        assert_eq!(span.linear(80), (70, 92));
    }

    /// Asserts that a detected URL's span keeps its first and last cells.
    ///
    /// Case: the pointer rests on a URL the hover system just detected.
    #[test]
    fn a_detected_url_converts_to_its_span() {
        let url = DetectedUrl {
            uri: "https://a.b".to_string(),
            first: cell(2, 3),
            last: cell(2, 13),
        };
        assert_eq!(
            DetectedSpan::from(&url),
            DetectedSpan {
                first: cell(2, 3),
                last: cell(2, 13),
            }
        );
    }
}
