//! Visible drag handles in tmux's reserved inter-pane gap, plus the resize
//! mouse cursor while hovering them.
//!
//! For each `DividerPixelRect` stored on a window (`PackedTmuxLayout`), one
//! handle node fills the 1px gap the divider occupies (subtle grey, brightening
//! to the accent color on hover). A hover system reuses the arbiter's
//! `divider_at` hit-test so the visible handle, the resize grab zone, and the
//! cursor all coincide, and sets a `ColResize` / `RowResize` cursor on the
//! primary window while the pointer is over a divider.

use crate::configs::OzmuxConfigsResource;
use crate::input::InputPhase;
use crate::theme;
use super::mouse::divider_at;
use super::render::{DividerPixelRect, PackedTmuxLayout};
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use ozmux_tmux::{ActiveWindow, TmuxProjectionSet};
use tmux_control_parser::DividerAxis;

/// Registers the divider-handle visuals and the resize hover cursor.
pub(crate) struct DividerHandlePlugin;

impl Plugin for DividerHandlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                reconcile_divider_handles.after(TmuxProjectionSet),
                divider_hover_feedback.after(InputPhase::Hover),
            ),
        );
    }
}

/// A visual bar filling one divider's reserved gap. `window` is the owning
/// `TmuxWindow` entity; stored so a layout change can despawn this window's
/// handles before respawning the new set.
#[derive(Component)]
struct DividerHandle {
    divider: DividerPixelRect,
    window: Entity,
}

/// Logical-px rect `(left, top, width, height)` for a divider's handle node.
fn handle_node_rect(d: &DividerPixelRect) -> (f32, f32, f32, f32) {
    match d.axis {
        DividerAxis::Vertical => (
            d.pos_px,
            d.span_start_px,
            1.0,
            d.span_end_px - d.span_start_px,
        ),
        DividerAxis::Horizontal => (
            d.span_start_px,
            d.pos_px,
            d.span_end_px - d.span_start_px,
            1.0,
        ),
    }
}

/// Despawns and respawns a window's handle bars whenever its `PackedTmuxLayout`
/// changes (first projection + every `%layout-change`). Spawns each handle with
/// its gap rect already set so it appears in place without a one-frame stutter.
fn reconcile_divider_handles(
    mut commands: Commands,
    changed: Query<(Entity, &PackedTmuxLayout), Changed<PackedTmuxLayout>>,
    handles: Query<(Entity, &DividerHandle)>,
) {
    if changed.is_empty() {
        return;
    }
    for (window_entity, packed) in changed.iter() {
        for (handle, handle_data) in handles.iter() {
            if handle_data.window == window_entity {
                commands.entity(handle).despawn();
            }
        }
        for divider in &packed.dividers {
            let (left, top, w, h) = handle_node_rect(divider);
            commands.spawn((
                DividerHandle {
                    divider: *divider,
                    window: window_entity,
                },
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(left),
                    top: Val::Px(top),
                    width: Val::Px(w),
                    height: Val::Px(h),
                    ..default()
                },
                BackgroundColor(theme::BORDER),
                ChildOf(window_entity),
            ));
        }
    }
}

/// Brightens the hovered divider's handle to the accent color and sets a
/// `ColResize` / `RowResize` cursor on the primary window. Runs after
/// `InputPhase::Hover` so it overrides the hyperlink baseline cursor; when no
/// divider is hovered it leaves the cursor for that baseline to reassert.
fn divider_hover_feedback(
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    mut handles: Query<(&DividerHandle, &mut BackgroundColor)>,
    packed_q: Query<&PackedTmuxLayout, With<ActiveWindow>>,
    configs: Option<Res<OzmuxConfigsResource>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let tol = configs
        .as_deref()
        .map(|c| c.mouse.divider_grab_tolerance_px)
        .unwrap_or(4.0);
    let dividers: &[DividerPixelRect] = packed_q
        .single()
        .map(|p| p.dividers.as_slice())
        .unwrap_or(&[]);
    let hovered = window
        .cursor_position()
        .and_then(|cursor| divider_at(dividers, cursor, tol));

    for (handle, mut bg) in handles.iter_mut() {
        let want = if hovered
            .is_some_and(|h| h.axis == handle.divider.axis && h.primary == handle.divider.primary)
        {
            theme::ACCENT
        } else {
            theme::BORDER
        };
        if bg.0 != want {
            bg.0 = want;
        }
    }

    let Some(divider) = hovered else {
        return;
    };
    let icon = match divider.axis {
        DividerAxis::Vertical => SystemCursorIcon::ColResize,
        DividerAxis::Horizontal => SystemCursorIcon::RowResize,
    };
    if let Ok(mut cursor) = cursor_icons.single_mut()
        && !matches!(&*cursor, CursorIcon::System(e) if *e == icon)
    {
        *cursor = CursorIcon::System(icon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::render::{DividerPixelRect, PackedTmuxLayout};
    use bevy::math::Vec2;
    use std::collections::HashMap;
    use tmux_control_parser::{DividerAxis, PaneId};

    #[test]
    fn reconcile_tracks_one_handle_per_divider() {
        let vdiv = |pos: f32| DividerPixelRect {
            axis: DividerAxis::Vertical,
            primary: PaneId(1),
            pos_px: pos,
            span_start_px: 0.0,
            span_end_px: 384.0,
        };
        let hdiv = |pos: f32| DividerPixelRect {
            axis: DividerAxis::Horizontal,
            primary: PaneId(1),
            pos_px: pos,
            span_start_px: 0.0,
            span_end_px: 640.0,
        };

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, reconcile_divider_handles);

        let window = app
            .world_mut()
            .spawn(PackedTmuxLayout {
                panes: HashMap::new(),
                dividers: vec![vdiv(320.0), hdiv(192.0)],
                bbox: Vec2::new(640.0, 384.0),
            })
            .id();
        app.update();

        let count = |app: &mut App| {
            app.world_mut()
                .query::<&DividerHandle>()
                .iter(app.world())
                .count()
        };
        assert_eq!(count(&mut app), 2, "one handle per divider");
        assert!(
            app.world_mut()
                .query::<&DividerHandle>()
                .iter(app.world())
                .all(|h| h.window == window),
            "handles belong to the dividers' window",
        );

        app.world_mut().entity_mut(window).insert(PackedTmuxLayout {
            panes: HashMap::new(),
            dividers: vec![vdiv(200.0)],
            bbox: Vec2::new(640.0, 384.0),
        });
        app.update();
        assert_eq!(count(&mut app), 1, "reconciled to the new divider set");

        app.world_mut()
            .entity_mut(window)
            .insert(PackedTmuxLayout::default());
        app.update();
        assert_eq!(count(&mut app), 0, "no dividers -> no handles");
    }
}
