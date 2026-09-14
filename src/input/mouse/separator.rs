//! Grabbing and dragging the divider between two panes.

use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy_orzmux::prelude::{OrzmuxSeparator, SplitId, SplitOrientation};

pub(super) struct SeparatorDragPlugin;

impl Plugin for SeparatorDragPlugin {
    fn build(&self, _app: &mut App) {}
}

/// The divider whose grab band contains a cursor position.
// NOTE: `#[expect(dead_code)]` reports an unfulfilled expectation here because
// only the test module constructs `SeparatorHit`; Task 10 adds the real
// caller and this attribute is removed then.
#[allow(
    dead_code,
    reason = "only tests construct this until Task 10 wires a caller"
)]
pub(crate) struct SeparatorHit {
    /// The separator entity the band belongs to.
    pub(crate) entity: Entity,
    /// The split the divider moves.
    pub(crate) split: SplitId,
    /// Which way the divider runs.
    pub(crate) orientation: SplitOrientation,
}

impl SeparatorHit {
    /// The divider whose grab band contains `cursor_phys`, in window
    /// physical px. Overlapping bands resolve to the nearer painted
    /// line, and an exact tie to the lower [`SplitId`]. `cell_px` is the
    /// `(width, height)` cell pitch in physical px.
    #[allow(
        dead_code,
        reason = "only tests call this until Task 10 wires a caller"
    )]
    pub(crate) fn resolve<'a>(
        cursor_phys: Vec2,
        scale: f32,
        cell_px: (f32, f32),
        separators: impl Iterator<
            Item = (
                Entity,
                &'a OrzmuxSeparator,
                &'a ComputedNode,
                &'a UiGlobalTransform,
            ),
        >,
    ) -> Option<Self> {
        let mut best: Option<(f32, SplitId, Self)> = None;
        for (entity, marker, node, transform) in separators {
            let centre = transform.translation;
            let pitch = match marker.orientation {
                SplitOrientation::Vertical => cell_px.0,
                SplitOrientation::Horizontal => cell_px.1,
            };
            let half_band = grab_half_band_phys(scale, pitch);
            let (across, along, half_len) = match marker.orientation {
                SplitOrientation::Vertical => (
                    (cursor_phys.x - centre.x).abs(),
                    (cursor_phys.y - centre.y).abs(),
                    node.size.y / 2.0,
                ),
                SplitOrientation::Horizontal => (
                    (cursor_phys.y - centre.y).abs(),
                    (cursor_phys.x - centre.x).abs(),
                    node.size.x / 2.0,
                ),
            };
            if across > half_band || along > half_len {
                continue;
            }
            let candidate = Self {
                entity,
                split: marker.split,
                orientation: marker.orientation,
            };
            let better = match &best {
                None => true,
                Some((best_across, best_split, _)) => {
                    across < *best_across || (across == *best_across && marker.split < *best_split)
                }
            };
            if better {
                best = Some((across, marker.split, candidate));
            }
        }
        best.map(|(_, _, hit)| hit)
    }
}

/// Half the grab band's thickness in logical px, measured from the
/// painted line's centre.
///
/// TODO: make the grab band configurable.
#[allow(
    dead_code,
    reason = "only `resolve` reads this until Task 10 wires a caller"
)]
const SEPARATOR_GRAB_HALF_BAND_LOGICAL_PX: f32 = 4.0;

/// Half the grab band in physical px: never below
/// [`SEPARATOR_GRAB_HALF_BAND_LOGICAL_PX`] logical px, and never below
/// half a cell so the band covers the visible groove.
#[allow(
    dead_code,
    reason = "only `resolve` calls this until Task 10 wires a caller"
)]
fn grab_half_band_phys(scale: f32, cell_pitch_phys: f32) -> f32 {
    (SEPARATOR_GRAB_HALF_BAND_LOGICAL_PX * scale).max(cell_pitch_phys / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALE: f32 = 1.0;
    const CELL: (f32, f32) = (8.0, 16.0);

    /// A vertical divider one physical px wide, centred at `x`, running
    /// from y=0 to y=`len_px`.
    fn vertical(x: f32, len_px: f32) -> (ComputedNode, UiGlobalTransform) {
        (
            ComputedNode {
                size: Vec2::new(1.0, len_px),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(x, len_px / 2.0),
        )
    }

    /// Asserts that a press within the band around the painted line
    /// resolves to that divider, and one beyond the band resolves to
    /// nothing.
    ///
    /// Case: the user aims at the visible groove between two panes, and
    /// then clicks well inside the pane next to it.
    #[test]
    fn a_press_inside_the_band_resolves_and_one_outside_does_not() {
        let marker = OrzmuxSeparator {
            split: SplitId(1),
            orientation: SplitOrientation::Vertical,
        };
        let (node, transform) = vertical(100.0, 400.0);

        let hit = SeparatorHit::resolve(
            Vec2::new(103.0, 200.0),
            SCALE,
            CELL,
            [(Entity::PLACEHOLDER, &marker, &node, &transform)].into_iter(),
        );
        assert_eq!(hit.map(|h| h.split), Some(SplitId(1)));

        let miss = SeparatorHit::resolve(
            Vec2::new(140.0, 200.0),
            SCALE,
            CELL,
            [(Entity::PLACEHOLDER, &marker, &node, &transform)].into_iter(),
        );
        assert!(miss.is_none());
    }

    /// Asserts that a press beyond the divider's own length misses it,
    /// so the band is bounded on both axes.
    ///
    /// Case: a short divider splits only the top half of the window and
    /// the user clicks in the full-height pane below it, on the same
    /// column the divider occupies.
    #[test]
    fn a_press_past_the_divider_length_misses_it() {
        let marker = OrzmuxSeparator {
            split: SplitId(1),
            orientation: SplitOrientation::Vertical,
        };
        let (node, transform) = vertical(100.0, 192.0);

        let hit = SeparatorHit::resolve(
            Vec2::new(100.0, 320.0),
            SCALE,
            CELL,
            [(Entity::PLACEHOLDER, &marker, &node, &transform)].into_iter(),
        );
        assert!(hit.is_none());
    }

    /// Asserts that overlapping bands resolve to the nearer painted
    /// line, and that an exact tie resolves to the lower split id.
    ///
    /// Case: the user aims between two vertical column dividers that
    /// sit five physical px apart.
    #[test]
    fn overlapping_bands_resolve_to_the_nearer_line_then_the_lower_id() {
        let near = OrzmuxSeparator {
            split: SplitId(2),
            orientation: SplitOrientation::Vertical,
        };
        let far = OrzmuxSeparator {
            split: SplitId(1),
            orientation: SplitOrientation::Vertical,
        };
        let (near_node, near_transform) = vertical(100.0, 400.0);
        let (far_node, far_transform) = vertical(105.0, 400.0);

        let hit = SeparatorHit::resolve(
            Vec2::new(101.0, 200.0),
            SCALE,
            CELL,
            [
                (Entity::PLACEHOLDER, &far, &far_node, &far_transform),
                (Entity::PLACEHOLDER, &near, &near_node, &near_transform),
            ]
            .into_iter(),
        );
        assert_eq!(hit.map(|h| h.split), Some(SplitId(2)));

        let (a_node, a_transform) = vertical(100.0, 400.0);
        let (b_node, b_transform) = vertical(100.0, 400.0);
        let tie = SeparatorHit::resolve(
            Vec2::new(100.0, 200.0),
            SCALE,
            CELL,
            [
                (Entity::PLACEHOLDER, &near, &a_node, &a_transform),
                (Entity::PLACEHOLDER, &far, &b_node, &b_transform),
            ]
            .into_iter(),
        );
        assert_eq!(tie.map(|h| h.split), Some(SplitId(1)));
    }
}
