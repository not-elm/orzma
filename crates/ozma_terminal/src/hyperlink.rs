//! Cmd/Ctrl-click hyperlink activation and hover-cursor feedback for the Ozma
//! terminal. The click-open path is invoked from the mouse dispatcher; the hover
//! system updates `HyperlinkHoverState` (renderer underline) and the window
//! `CursorIcon`.

use crate::input::{InputDisabled, current_terminal_modifiers};
use crate::mouse::cell_at_cursor;
use crate::spawn::OzmaTerminal;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon, Window};
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

/// The cursor icon for a hover state: pointer over a link with the modifier
/// held, I-beam over the grid, arrow elsewhere.
pub(crate) fn cursor_decision(
    has_link: bool,
    modifier_held: bool,
    over_grid: bool,
) -> SystemCursorIcon {
    match (over_grid, has_link, modifier_held) {
        (true, true, true) => SystemCursorIcon::Pointer,
        (true, _, _) => SystemCursorIcon::Text,
        _ => SystemCursorIcon::Default,
    }
}

/// Updates `HyperlinkHoverState` and the window cursor as the pointer moves over
/// the terminal grid. Gated to the single enabled `OzmaTerminal`.
pub(crate) fn hyperlink_hover_cursor(
    mut hover: ResMut<HyperlinkHoverState>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    terminal: Query<
        (Entity, &ComputedNode, &UiGlobalTransform, &TerminalGrid),
        (With<OzmaTerminal>, Without<InputDisabled>),
    >,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let modifier_held = link_modifier_held(&{
        let m = current_terminal_modifiers(&keys);
        ProtocolModifiers { shift: m.shift, ctrl: m.ctrl, alt: m.alt, meta: m.meta }
    });
    hover.entity = None;
    hover.hyperlink_id = None;
    hover.modifier_held = modifier_held;

    let decision = resolve_hover(&mut hover, &terminal, &metrics, &windows, modifier_held);
    if let Ok(mut icon) = cursor_icons.single_mut() {
        let desired = CursorIcon::System(decision);
        if *icon != desired {
            *icon = desired;
        }
    }
}

fn resolve_hover(
    hover: &mut HyperlinkHoverState,
    terminal: &Query<
        (Entity, &ComputedNode, &UiGlobalTransform, &TerminalGrid),
        (With<OzmaTerminal>, Without<InputDisabled>),
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
    let Ok((entity, node, transform, grid)) = terminal.single() else {
        return SystemCursorIcon::Default;
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let Some((cell, _side)) =
        cell_at_cursor(node, transform, cursor_phys, cell_w, cell_h, grid.cols, grid.rows)
    else {
        return SystemCursorIcon::Default;
    };
    let id = grid
        .hyperlink_at((cell.row - 1) as u16, (cell.col - 1) as u16)
        .map(|(id, _uri)| id);
    hover.entity = Some(entity);
    hover.hyperlink_id = id;
    cursor_decision(id.is_some(), modifier_held, true)
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
        assert_eq!(cursor_decision(true, true, true), SystemCursorIcon::Pointer);
        assert_eq!(cursor_decision(true, false, true), SystemCursorIcon::Text);
        assert_eq!(cursor_decision(false, true, true), SystemCursorIcon::Text);
        assert_eq!(cursor_decision(false, false, false), SystemCursorIcon::Default);
    }
}
