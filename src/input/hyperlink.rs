//! OSC 8 hyperlink hover detection, cursor-icon control, scheme
//! allowlist, and Cmd+click activation. The plugin registered here
//! also re-exports the pure predicates the mouse-buttons system calls
//! during interception.

use bevy::ecs::entity::Entity;
use bevy::input::ButtonInput;
use bevy::input::keyboard::KeyCode;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon, Window};
use bevy_terminal_renderer::TerminalCellMetricsResource;
use bevy_terminal_renderer::schema::{HyperlinkHoverState, TerminalGrid};
use ozmux_configs::shortcuts::Modifiers;

/// Plugin: registers `hyperlink_hover_and_cursor` in `InputPhase::Hover`.
pub(crate) struct HyperlinkInputPlugin;

impl Plugin for HyperlinkInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            hyperlink_hover_and_cursor.in_set(crate::input::InputPhase::Hover),
        );
    }
}

/// Returns `true` when the platform's hyperlink-activation modifier is
/// currently held: Cmd (`meta`) on macOS, Ctrl elsewhere.
pub(crate) fn link_modifier_held(mods: &Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        mods.meta
    } else {
        mods.ctrl
    }
}

fn hyperlink_hover_and_cursor(
    mut hover: ResMut<HyperlinkHoverState>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    hosts: Query<
        (Entity, &ComputedNode, &UiGlobalTransform),
        With<crate::ui::ActivityHostNode>,
    >,
    grids: Query<&TerminalGrid>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Ok(window) = windows.single() else {
        clear_hover(&mut hover, &mut cursor_icons);
        return;
    };
    let scale = window.scale_factor();
    let Some(cursor_logical) = window.cursor_position() else {
        clear_hover(&mut hover, &mut cursor_icons);
        return;
    };
    let cursor_phys = cursor_logical * scale;
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);

    let mods = crate::input::current_modifiers(&keys);
    hover.modifier_held = link_modifier_held(&mods);

    let Some((entity, local)) =
        crate::input::mouse_buttons::resolve_pane_at_phys(&hosts, cursor_phys)
    else {
        hover.entity = None;
        hover.hyperlink_id = None;
        write_cursor_icon(&mut cursor_icons, SystemCursorIcon::Text);
        return;
    };
    let Ok(grid) = grids.get(entity) else {
        hover.entity = None;
        hover.hyperlink_id = None;
        write_cursor_icon(&mut cursor_icons, SystemCursorIcon::Text);
        return;
    };
    let (col, row, _side) = crate::input::mouse_buttons::cell_at_local(
        local,
        cell_w_phys,
        cell_h_phys,
        grid.cols,
        grid.rows,
    );
    // NOTE: cell_at_local returns 1-indexed coords; convert to 0-indexed
    //       for the grid lookup. Overlooking this misaligns hover by one cell.
    let id = grid
        .hyperlink_at(row.saturating_sub(1) as u16, col.saturating_sub(1) as u16)
        .map(|(id, _uri)| id);
    hover.entity = Some(entity);
    hover.hyperlink_id = id;

    let desired = if id.is_some() && hover.modifier_held {
        SystemCursorIcon::Pointer
    } else {
        SystemCursorIcon::Text
    };
    write_cursor_icon(&mut cursor_icons, desired);
}

fn clear_hover(
    hover: &mut HyperlinkHoverState,
    cursor_icons: &mut Query<&mut CursorIcon, With<PrimaryWindow>>,
) {
    hover.entity = None;
    hover.hyperlink_id = None;
    hover.modifier_held = false;
    write_cursor_icon(cursor_icons, SystemCursorIcon::Text);
}

fn write_cursor_icon(
    cursor_icons: &mut Query<&mut CursorIcon, With<PrimaryWindow>>,
    desired: SystemCursorIcon,
) {
    let Ok(mut icon) = cursor_icons.single_mut() else {
        return;
    };
    // NOTE: idempotent write — only mutate when the desired value differs
    // from the current one so winit's `update_cursors` does not fire
    // `Changed<CursorIcon>` every frame.
    let already = match &*icon {
        CursorIcon::System(existing) => *existing == desired,
        _ => false,
    };
    if !already {
        *icon = CursorIcon::System(desired);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Modifiers {
        Modifiers::default()
    }

    #[test]
    fn link_modifier_held_returns_false_when_no_modifier() {
        assert!(!link_modifier_held(&empty()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn link_modifier_held_macos_requires_meta() {
        let mut mods = empty();
        mods.ctrl = true;
        assert!(!link_modifier_held(&mods));
        mods.meta = true;
        assert!(link_modifier_held(&mods));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn link_modifier_held_non_macos_requires_ctrl() {
        let mut mods = empty();
        mods.meta = true;
        assert!(!link_modifier_held(&mods));
        mods.ctrl = true;
        assert!(link_modifier_held(&mods));
    }
}
