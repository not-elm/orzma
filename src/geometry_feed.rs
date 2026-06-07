//! Reports the window-driven workspace viewport size (in cells) to the Mux —
//! the single authoritative geometry input (window-following policy). The Mux
//! then resolves per-pane cells (PaneResized → PaneDimensions).
//!
//! The measured node is the `WorkspaceUiRoot` entity: it is sized 100 % of the
//! available window area by CSS (`flex_grow: 1`, `width/height: Percent(100)`)
//! and is never resized by the Task-3 render system, so measuring it cannot
//! create a feedback loop.

use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy_terminal_renderer::TerminalCellMetricsResource;
#[cfg(not(feature = "thin-client"))]
use ozmux_multiplexer::MultiplexerCommands;
use ozmux_multiplexer::{AttachedWorkspace, WorkspaceDimensions};

use crate::ui::WorkspaceUiRoot;

/// Bevy plugin that registers the `feed_workspace_dimensions` system.
pub struct GeometryFeedPlugin;

impl Plugin for GeometryFeedPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            feed_workspace_dimensions
                .after(UiSystems::Layout)
                .before(UiSystems::PostLayout),
        );
    }
}

/// Measures the `WorkspaceUiRoot` node (window-driven, 100 % of the available
/// window area), converts the physical-pixel size to a cell grid using the
/// floored physical cell pitch from `TerminalCellMetricsResource`, and calls
/// `mux.set_workspace_dimensions` for the attached workspace.
///
/// Change guard: skips the call when the computed `(cols, rows)` pair matches
/// the current `WorkspaceDimensions` component already on the workspace entity,
/// avoiding re-emitting `PaneResized` events on every frame when nothing has
/// changed.
#[cfg(not(feature = "thin-client"))]
fn feed_workspace_dimensions(
    mut mux: MultiplexerCommands,
    workspace_ui_root: Query<&ComputedNode, With<WorkspaceUiRoot>>,
    attached: Query<(Entity, Option<&WorkspaceDimensions>), With<AttachedWorkspace>>,
    metrics: Res<TerminalCellMetricsResource>,
) {
    let Ok(computed) = workspace_ui_root.single() else {
        return;
    };
    let Ok((workspace, current_dims)) = attached.single() else {
        return;
    };

    let phys_w = computed.size.x.max(0.0);
    let phys_h = computed.size.y.max(0.0);
    if phys_w == 0.0 || phys_h == 0.0 {
        return;
    }

    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let cols = ((phys_w / cell_w).floor() as u16).max(1);
    let rows = ((phys_h / cell_h).floor() as u16).max(1);

    if let Some(dims) = current_dims
        && dims.cols == cols
        && dims.rows == rows
    {
        return;
    }

    mux.set_workspace_dimensions(workspace, cols, rows);
}

/// Thin-client variant: sends `SetViewport` over the wire instead of calling
/// `MultiplexerCommands::set_workspace_dimensions`. Same cols/rows computation
/// and `WorkspaceDimensions` change-guard as the local variant.
#[cfg(feature = "thin-client")]
fn feed_workspace_dimensions(
    mut commands: Commands,
    mut conn: NonSendMut<crate::thin_client::ThinClientConn>,
    workspace_ui_root: Query<&ComputedNode, With<WorkspaceUiRoot>>,
    attached: Query<(Entity, Option<&WorkspaceDimensions>), With<AttachedWorkspace>>,
    metrics: Res<TerminalCellMetricsResource>,
) {
    let Ok(computed) = workspace_ui_root.single() else {
        return;
    };
    let Ok((workspace, current_dims)) = attached.single() else {
        return;
    };

    let phys_w = computed.size.x.max(0.0);
    let phys_h = computed.size.y.max(0.0);
    if phys_w == 0.0 || phys_h == 0.0 {
        return;
    }

    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let cols = ((phys_w / cell_w).floor() as u16).max(1);
    let rows = ((phys_h / cell_h).floor() as u16).max(1);

    if let Some(dims) = current_dims
        && dims.cols == cols
        && dims.rows == rows
    {
        return;
    }

    if let Err(e) = conn
        .0
        .send(ozmux_proto::ClientMessage::SetViewport { cols, rows })
    {
        bevy::log::error!("thin-client: SetViewport send failed: {e}");
        return;
    }
    commands
        .entity(workspace)
        .insert(WorkspaceDimensions { cols, rows });
}
