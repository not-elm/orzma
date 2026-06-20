//! Cmd/Ctrl-click hyperlink activation and hover-cursor feedback for the Ozma
//! terminal. The click-open path is invoked from the mouse dispatcher; the hover
//! system updates `HyperlinkHoverState` (renderer underline) and the window
//! `CursorIcon`.

use crate::mouse::{
    MouseDisabled, OzmaTerminalMouseSet, cell_at_cursor, protocol_mods, topmost_terminal_at,
};
use crate::spawn::OzmaTerminal;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorIcon, CursorMoved, PrimaryWindow, SystemCursorIcon, Window};
use ozma_tty_engine::ProtocolModifiers;
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::schema::{HyperlinkHoverState, TerminalGrid, is_allowed};

/// Returns `true` when the platform link-activation modifier is held: Cmd on
/// macOS, Ctrl elsewhere.
pub(crate) fn link_modifier_held(mods: &ProtocolModifiers) -> bool {
    if cfg!(target_os = "macos") {
        mods.meta
    } else {
        mods.ctrl
    }
}

/// Validates `uri` against the shared allowlist and opens it via the OS default
/// handler. Disallowed URIs are dropped with a debug log.
pub(crate) fn try_open_uri(uri: &str) {
    if !is_allowed(uri) {
        debug!("hyperlink: dropping disallowed uri {}", uri);
        return;
    }
    if let Err(e) = open::that_detached(uri) {
        warn!("hyperlink: failed to open {}: {}", uri, e);
    }
}

/// Registers the terminal's hyperlink hover-cursor feedback.
///
/// Adds `hyperlink_hover_cursor` to `OzmaTerminalMouseSet`, gated to run only
/// when the pointer moves or a key is pressed. The click-open path lives in the
/// mouse dispatcher, not here.
pub(crate) struct HyperlinkPlugin;

impl Plugin for HyperlinkPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<CursorMoved>().add_systems(
            Update,
            hyperlink_hover_cursor
                .in_set(OzmaTerminalMouseSet)
                .run_if(on_message::<KeyboardInput>.or(on_message::<CursorMoved>)),
        );
    }
}

/// Updates `HyperlinkHoverState` and the window cursor as the pointer moves over
/// the terminal grid. Resolves the hover against the topmost enabled
/// `OzmaTerminal` under the cursor.
fn hyperlink_hover_cursor(
    mut hover: ResMut<HyperlinkHoverState>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    terminals: Query<
        (Entity, &ComputedNode, &UiGlobalTransform, &TerminalGrid),
        (With<OzmaTerminal>, Without<MouseDisabled>),
    >,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let modifier_held = link_modifier_held(&protocol_mods(&keys));
    hover.entity = None;
    hover.hyperlink_id = None;
    hover.modifier_held = modifier_held;

    let decision = resolve_hover(&mut hover, &terminals, &metrics, &windows, modifier_held);
    if let Ok(mut icon) = cursor_icons.single_mut() {
        let desired = CursorIcon::System(decision);
        if *icon != desired {
            *icon = desired;
        }
    }
}

/// The cursor icon over the grid: pointer over a link with the modifier held,
/// otherwise the I-beam. Off-grid cases return `Default` in `resolve_hover`
/// before this is reached.
fn cursor_decision(has_link: bool, modifier_held: bool) -> SystemCursorIcon {
    if has_link && modifier_held {
        SystemCursorIcon::Pointer
    } else {
        SystemCursorIcon::Text
    }
}

fn resolve_hover(
    hover: &mut HyperlinkHoverState,
    terminals: &Query<
        (Entity, &ComputedNode, &UiGlobalTransform, &TerminalGrid),
        (With<OzmaTerminal>, Without<MouseDisabled>),
    >,
    metrics: &TerminalCellMetricsResource,
    windows: &Query<&Window, With<PrimaryWindow>>,
    modifier_held: bool,
) -> SystemCursorIcon {
    let Ok(window) = windows.single() else {
        return SystemCursorIcon::Default;
    };
    let Some(cursor_phys) = window.cursor_position().map(|c| c * window.scale_factor()) else {
        return SystemCursorIcon::Default;
    };
    let Some(target) = topmost_terminal_at(
        cursor_phys,
        terminals
            .iter()
            .map(|(e, node, transform, _)| (e, node, transform)),
    ) else {
        return SystemCursorIcon::Default;
    };
    let Ok((entity, node, transform, grid)) = terminals.get(target) else {
        return SystemCursorIcon::Default;
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let Some((cell, _side)) = cell_at_cursor(
        node,
        transform,
        cursor_phys,
        cell_w,
        cell_h,
        grid.cols,
        grid.rows,
    ) else {
        return SystemCursorIcon::Default;
    };
    let id = grid
        .hyperlink_at((cell.row - 1) as u16, (cell.col - 1) as u16)
        .map(|(id, _uri)| id);
    hover.entity = Some(entity);
    hover.hyperlink_id = id;
    cursor_decision(id.is_some(), modifier_held)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_modifier_matches_platform() {
        let mut m = ProtocolModifiers::default();
        assert!(!link_modifier_held(&m));
        if cfg!(target_os = "macos") {
            m.meta = true;
        } else {
            m.ctrl = true;
        }
        assert!(link_modifier_held(&m));
    }

    #[test]
    fn cursor_decision_pointer_only_on_link_with_modifier() {
        assert_eq!(cursor_decision(true, true), SystemCursorIcon::Pointer);
        assert_eq!(cursor_decision(false, true), SystemCursorIcon::Text);
        assert_eq!(cursor_decision(true, false), SystemCursorIcon::Text);
    }
}
