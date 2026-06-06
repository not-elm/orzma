//! Structural types and spawn-bundle helpers for the entity-based layout tree.
//! Owns `SplitOrientation`, `Side`, `split_node_bundle`, and `pane_frame_node`.

use crate::components::SplitNode;
use bevy::prelude::*;
use bevy::ui::{FlexDirection, Val};

/// Split axis.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum SplitOrientation {
    /// Left and right children share horizontal space.
    Horizontal,
    /// Top and bottom children share vertical space.
    Vertical,
}

/// Which side of an existing cell a newly-inserted sibling lands on.
#[derive(Debug, Default, Clone, Copy, Hash, Eq, PartialEq)]
pub enum Side {
    /// Place the new node before the target (left or top).
    Before,
    /// Place the new node after the target (right or bottom).
    #[default]
    After,
}

/// The `Node` + `SplitNode` pair for a split container at `orientation`.
///
/// Both axes are `Val::Auto` so taffy fits the container to the sum of its
/// fixed-px children (pane leaves sized by `size_pane_leaves` each frame).
/// The top-level split is the only child of the 100 %/100 % layout-root
/// container, which anchors the whole tile tree to the viewport. Nested
/// splits compose correctly because each child's px size is Mux-determined
/// and the children always sum to the parent's slot.
pub(crate) fn split_node_bundle(orientation: SplitOrientation) -> (Node, SplitNode) {
    let flex_direction = match orientation {
        SplitOrientation::Horizontal => FlexDirection::Row,
        SplitOrientation::Vertical => FlexDirection::Column,
    };
    // NOTE: Val::Auto lets taffy derive both axes from the fixed-px children.
    // A Horizontal split's width = sum(children widths), height = children
    // height; a Vertical split's height = sum(children heights), width =
    // children width.  This is correct because the Mux guarantees children
    // cells sum exactly to the parent slot.
    (
        Node {
            flex_direction,
            width: Val::Auto,
            height: Val::Auto,
            ..default()
        },
        SplitNode { orientation },
    )
}

/// The `Node` for a Pane frame (flex column; width/height are overwritten each
/// frame by the `size_pane_leaves` render system to the Mux-resolved cell rect
/// in logical px).
pub(crate) fn pane_frame_node() -> Node {
    Node {
        flex_direction: FlexDirection::Column,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        ..default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_node_bundle_sets_flex_direction_from_orientation() {
        let (node, split) = split_node_bundle(SplitOrientation::Vertical);
        assert_eq!(node.flex_direction, FlexDirection::Column);
        assert_eq!(split.orientation, SplitOrientation::Vertical);
        let (node_h, _) = split_node_bundle(SplitOrientation::Horizontal);
        assert_eq!(node_h.flex_direction, FlexDirection::Row);
    }

    #[test]
    fn split_node_bundle_uses_auto_sizing() {
        let (node, _) = split_node_bundle(SplitOrientation::Horizontal);
        assert_eq!(node.width, Val::Auto, "split container width must be Auto");
        assert_eq!(
            node.height,
            Val::Auto,
            "split container height must be Auto"
        );
    }
}
