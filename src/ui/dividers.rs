//! Overlay divider lines at split boundaries.
//!
//! For each `SplitNode` entity the system ensures exactly one flat
//! `SplitDivider` node exists under `WorkspaceUiRoot`. The divider is 1 px
//! wide (horizontal split) or 1 px tall (vertical split), positioned
//! absolutely in window coordinates at the split boundary. It carries
//! `Pickable::IGNORE` so it never intercepts pointer events.
//!
//! The system runs in `PostUpdate` **after** `UiSystems::Layout` so the
//! split's first child's `ComputedNode` and `UiGlobalTransform` are
//! current for the current frame.
//!
//! Dividers are NOT children of the split entity. The multiplexer's
//! `assert_mirror_consistent` enforces that every `SplitNode` has exactly
//! two children; adding a third would trip that invariant and panic in
//! tests. Instead, dividers live under `WorkspaceUiRoot` (a flat
//! sibling of the layout tree) and use `PositionType::Absolute` with
//! window-absolute coordinates derived from the first child's
//! `UiGlobalTransform` and `ComputedNode`.
//!
//! NOTE: `UiGlobalTransform.translation` is the CENTER of the node in
//! PHYSICAL pixels. `ComputedNode.size()` is also physical px. Multiply
//! by `inverse_scale_factor()` to convert to logical px for `Val::Px`.
//! This matches the pattern in `src/extension_render.rs`.

use crate::ui::{WorkspaceUiRoot, palette};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, GlobalZIndex, PositionType, UiGlobalTransform, UiSystems, Val};
use ozmux_multiplexer::{SplitNode, SplitOrientation};

/// Z-index for split dividers — above pane content (terminal/webview layers
/// sit at 0) but below the IME overlay (200).
const DIVIDER_Z: i32 = 10;

/// Bevy plugin that registers the divider overlay system.
pub(crate) struct SplitDividerPlugin;

impl Plugin for SplitDividerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, sync_split_dividers.after(UiSystems::Layout));
    }
}

/// Marker on the 1-px divider node owned by a split container.
///
/// `split` is the owning `SplitNode` entity; used to detect stale dividers
/// whose split has been despawned.
#[derive(Component)]
pub(crate) struct SplitDivider {
    split: Entity,
}

/// Ensures each `SplitNode` has a correctly-positioned `SplitDivider` under
/// `WorkspaceUiRoot`, and despawns dividers whose owning split no longer
/// exists in the `splits` query.
///
/// Positioning: reads the first non-divider child's `UiGlobalTransform`
/// (center, physical px) and `ComputedNode` (size, physical px) to derive
/// the split boundary in window-absolute logical px.
fn sync_split_dividers(
    mut commands: Commands,
    splits: Query<(Entity, &SplitNode, &Children)>,
    mut dividers: Query<(Entity, &SplitDivider, &mut Node)>,
    anchors: Query<(&ComputedNode, &UiGlobalTransform), Without<SplitDivider>>,
    workspace_ui_root: Query<Entity, With<WorkspaceUiRoot>>,
) {
    let Ok(root) = workspace_ui_root.single() else {
        return;
    };

    let mut visited_splits: std::collections::HashSet<Entity> = std::collections::HashSet::new();

    for (split_entity, split_node, children) in splits.iter() {
        visited_splits.insert(split_entity);

        // The first non-divider child is the "left" or "top" pane/split
        // whose far edge is the boundary.
        let first_child = children.iter().find(|c| !dividers.contains(*c));

        let Some(first_child_entity) = first_child else {
            continue;
        };
        let Ok((computed, xform)) = anchors.get(first_child_entity) else {
            continue;
        };

        // Convert physical px → logical px.
        let inv = computed.inverse_scale_factor();
        let size_phys = computed.size();
        let center_phys = xform.translation;

        // Top-left of the first child in logical px.
        let top_left_logical = (center_phys - 0.5 * size_phys) * inv;

        let (left, top, width, height) = match split_node.orientation {
            // Horizontal split: divider is a vertical 1 px line at the right
            // edge of the first child (= left edge of the second child).
            SplitOrientation::Horizontal => (
                Val::Px((center_phys.x + 0.5 * size_phys.x) * inv),
                Val::Px(top_left_logical.y),
                Val::Px(1.0),
                Val::Px(size_phys.y * inv),
            ),
            // Vertical split: divider is a horizontal 1 px line at the bottom
            // edge of the first child.
            SplitOrientation::Vertical => (
                Val::Px(top_left_logical.x),
                Val::Px((center_phys.y + 0.5 * size_phys.y) * inv),
                Val::Px(size_phys.x * inv),
                Val::Px(1.0),
            ),
        };

        // Find the existing divider for this split, if any.
        let existing = dividers
            .iter()
            .find(|(_, d, _)| d.split == split_entity)
            .map(|(e, _, _)| e);

        if let Some(div) = existing {
            if let Ok((_, _, mut node)) = dividers.get_mut(div) {
                if node.left != left {
                    node.left = left;
                }
                if node.top != top {
                    node.top = top;
                }
                if node.width != width {
                    node.width = width;
                }
                if node.height != height {
                    node.height = height;
                }
            }
        } else {
            commands.spawn((
                Name::new(format!("SplitDivider({split_entity:?})")),
                Node {
                    position_type: PositionType::Absolute,
                    left,
                    top,
                    width,
                    height,
                    ..default()
                },
                BackgroundColor(palette::BORDER),
                GlobalZIndex(DIVIDER_Z),
                Pickable::IGNORE,
                SplitDivider {
                    split: split_entity,
                },
                // NOTE: dividers are absolute-positioned in `WorkspaceUiRoot`'s
                // coordinate space, which must stay anchored at the window
                // origin (0,0): it is the first child of the Column `UiRoot` and
                // the status bar appends BELOW it. Never insert a preceding
                // sibling under `UiRoot`, or every divider shifts.
                ChildOf(root),
            ));
        }
    }

    // Despawn dividers whose split entity is no longer visited.
    let stale: Vec<Entity> = dividers
        .iter()
        .filter(|(_, d, _)| !visited_splits.contains(&d.split))
        .map(|(e, _, _)| e)
        .collect();
    for e in stale {
        commands.entity(e).despawn();
    }
}
