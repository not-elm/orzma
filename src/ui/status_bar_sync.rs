//! Standalone status-bar rebuild system. Rebuilds when the set of Workspace
//! entities changes (Added/RemovedComponents on WorkspaceMarker) or the
//! AttachedWorkspace marker moves. Does NOT depend on per-workspace epoch
//! bumps — content changes (split / surface-add) do not redraw the
//! status bar.

use crate::font::TerminalUiFont;
use crate::ui::UiRoot;
use crate::ui::status_bar::build_status_bar;
use bevy::prelude::*;
use ozmux_multiplexer::{AttachedWorkspace, WorkspaceCreatedAt, WorkspaceMarker};

/// Marker on the currently-active status bar root Node. `build_status_bar`
/// inserts this on the bar entity it spawns; the standalone rebuild
/// system queries this to find and despawn the previous bar before
/// spawning a replacement.
#[derive(Component)]
pub struct StatusBarRoot;

/// Despawns the existing `StatusBarRoot` and rebuilds via
/// `crate::ui::status_bar::build_status_bar` when:
/// - any `WorkspaceMarker` was added or removed this frame, OR
/// - any `AttachedWorkspace` marker was added or removed this frame.
pub fn rebuild_status_bar_on_workspace_set_change(
    mut commands: Commands,
    mut attached_removed: RemovedComponents<AttachedWorkspace>,
    mut workspaces_removed: RemovedComponents<WorkspaceMarker>,
    workspaces: Query<
        (
            Entity,
            &Name,
            Has<AttachedWorkspace>,
            Option<&WorkspaceCreatedAt>,
        ),
        With<WorkspaceMarker>,
    >,
    ui_root: Query<Entity, With<UiRoot>>,
    status_bar: Query<Entity, With<StatusBarRoot>>,
    workspaces_added: Query<(), Added<WorkspaceMarker>>,
    attached_added: Query<(), Added<AttachedWorkspace>>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    let any_workspace_added = workspaces_added.iter().count() > 0;
    let any_workspace_removed = workspaces_removed.read().count() > 0;
    let any_attached_added = attached_added.iter().count() > 0;
    let any_attached_removed = attached_removed.read().count() > 0;

    if !(any_workspace_added || any_workspace_removed || any_attached_added || any_attached_removed)
    {
        return;
    }

    let Ok(ui_root) = ui_root.single() else {
        return;
    };

    for e in status_bar.iter() {
        commands.entity(e).try_despawn();
    }

    // Sort by `WorkspaceCreatedAt` (monotonic from `WorkspaceNameCounter`)
    // rather than by `Entity`: Bevy's entity allocator does not guarantee
    // strictly monotonic indices across multiple deferred command queues,
    // so an Entity-based sort would not match workspace creation order.
    // Externally-spawned workspaces without `WorkspaceCreatedAt` sort last via
    // the `u32::MAX` fallback.
    let mut workspaces: Vec<(Entity, String, bool, u32)> = workspaces
        .iter()
        .map(|(e, name, attached, created)| {
            (
                e,
                name.as_str().to_string(),
                attached,
                created.map(|c| c.0).unwrap_or(u32::MAX),
            )
        })
        .collect();
    workspaces.sort_by_key(|(_, _, _, created_at)| *created_at);
    let workspaces: Vec<(Entity, String, bool)> = workspaces
        .into_iter()
        .map(|(e, name, attached, _)| (e, name, attached))
        .collect();

    let ui_font_handle = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();
    build_status_bar(&mut commands, ui_root, &workspaces, &ui_font_handle);
}
