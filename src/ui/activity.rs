//! Activity host Bevy UI builder. The host entity itself is owned by
//! `ActivityEntityRegistry` and lives across structural rebuilds; this
//! module's `build_activity_host_children` populates its children (the
//! placeholder Row + name Text) on each rebuild.
//!
//! Phase 3+ replaces these placeholder children with a `MaterialNode<
//! TerminalMaterial>` for the GPU terminal grid.

use crate::ui::StructuralNode;
use crate::ui::palette;
use bevy::color::Color;
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, JustifyContent, Val};
use ozmux_multiplexer::ActivityKind;

/// Background color for the Activity placeholder host, chosen by kind.
fn kind_color(kind: &ActivityKind) -> Color {
    match kind {
        ActivityKind::Terminal => palette::ACTIVITY_TERMINAL,
        ActivityKind::Browser { .. } => palette::ACTIVITY_BROWSER,
        ActivityKind::Extension { .. } => palette::ACTIVITY_EXTENSION,
    }
}

/// Insert / refresh the `Node` bundle on the stable Activity host entity,
/// then spawn its (structural, replaced each rebuild) placeholder children
/// showing the activity name. The host's `Node` is set with
/// `commands.entity(host).insert(...)` so the existing entity remains;
/// children are spawned fresh.
///
/// For `ActivityKind::Terminal` and `ActivityKind::Extension` the
/// placeholder children are skipped because a full-size `MaterialNode`
/// (`TerminalUiMaterial` / `WebviewUiMaterial`) covers the host node
/// entirely. The kind-colored `BackgroundColor` still shows briefly
/// between activity creation and material readiness.
pub(crate) fn build_activity_host_children(
    commands: &mut Commands,
    host: Entity,
    kind: &ActivityKind,
    name: &Name,
) {
    commands.entity(host).insert((
        Node {
            flex_grow: 1.0,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(kind_color(kind)),
    ));

    if matches!(
        kind,
        ActivityKind::Terminal | ActivityKind::Extension { .. }
    ) {
        return;
    }

    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                ..default()
            },
            StructuralNode,
            ChildOf(host),
        ))
        .id();

    commands.spawn((
        Text::new(name.as_str().to_string()),
        TextColor(palette::FOREGROUND),
        StructuralNode,
        ChildOf(row),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_multiplexer::BrowserProfile;

    #[test]
    fn kind_color_terminal_uses_activity_terminal_constant() {
        assert_eq!(
            kind_color(&ActivityKind::Terminal),
            palette::ACTIVITY_TERMINAL
        );
    }

    #[test]
    fn kind_color_browser_uses_activity_browser_constant() {
        let kind = ActivityKind::Browser {
            initial_url: None,
            profile: BrowserProfile::default(),
        };
        assert_eq!(kind_color(&kind), palette::ACTIVITY_BROWSER);
    }
}
