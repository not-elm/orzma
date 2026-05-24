//! Activity host Bevy UI builder. The host entity itself is owned by
//! `ActivityEntityRegistry` and lives across structural rebuilds; this
//! module's `build_activity_host_children` populates its children (the
//! placeholder Row + name + short-id Texts) on each rebuild.
//!
//! Phase 3+ replaces these placeholder children with a `MaterialNode<
//! TerminalMaterial>` for the GPU terminal grid.

use crate::ui::StructuralNode;
use crate::ui::palette;
use bevy::color::Color;
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, JustifyContent, Val};
use ozmux_multiplexer::{Activity, ActivityId, ActivityKind};

/// Background color for the Activity placeholder host, chosen by kind.
fn kind_color(kind: &ActivityKind) -> Color {
    match kind {
        ActivityKind::Terminal => palette::ACTIVITY_TERMINAL,
        ActivityKind::Browser { .. } => palette::ACTIVITY_BROWSER,
        ActivityKind::Extension { .. } => palette::ACTIVITY_EXTENSION,
    }
}

/// Insert / refresh the Node bundle on the stable Activity host entity,
/// then spawn its (structural, replaced each rebuild) children showing
/// the activity name + short id. The host's `Node` is set with
/// `commands.entity(host).insert(...)` so the existing entity remains;
/// children are spawned fresh.
///
/// For `ActivityKind::Terminal` the placeholder children are skipped
/// because the renderer-side `MaterialNode<TerminalUiMaterial>` covers
/// the host node entirely. The terminal-colored `BackgroundColor` still
/// shows briefly between activity creation and material readiness, which
/// is useful when spawning fails.
pub(crate) fn build_activity_host_children(
    commands: &mut Commands,
    host: Entity,
    activity: &Activity,
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
        BackgroundColor(kind_color(&activity.kind)),
    ));

    if matches!(activity.kind, ActivityKind::Terminal) {
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
        Text::new(activity.name.clone()),
        TextColor(palette::FOREGROUND),
        StructuralNode,
        ChildOf(row),
    ));

    commands.spawn((
        Text::new(short_id(&activity.id).to_string()),
        TextColor(palette::FOREGROUND),
        StructuralNode,
        ChildOf(row),
    ));
}

/// First 8 bytes of an `ActivityId`'s UUID string (UUID v4 is always 36 ASCII chars).
fn short_id(id: &ActivityId) -> &str {
    &AsRef::<str>::as_ref(id)[..8]
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
