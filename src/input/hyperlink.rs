//! OSC 8 hyperlink hover detection, cursor-icon control, scheme
//! allowlist, and Cmd+click activation. The plugin registered here
//! also re-exports the pure predicates the mouse-buttons system calls
//! during interception.

use crate::input::mouse_buttons::{cell_at_local, resolve_pane_at_phys};
use crate::input::{InputPhase, current_modifiers};
use crate::ui::ActivityHostNode;
use bevy::ecs::entity::Entity;
use bevy::input::ButtonInput;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::MouseMotion;
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
            hyperlink_hover_and_cursor
                .run_if(on_message::<MouseMotion>.or(on_message::<KeyboardInput>))
                .in_set(InputPhase::Hover),
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

/// Validates `uri` against the scheme allowlist and hands it to the
/// OS default opener via `open::that_detached`. Disallowed URIs are
/// dropped with a debug log; opener errors are warned.
pub(crate) fn try_open_uri(uri: &str) {
    if !is_allowed(uri) {
        debug!("hyperlink: dropping disallowed uri {}", uri);
        return;
    }
    if let Err(e) = open::that_detached(uri) {
        warn!("hyperlink: failed to open {}: {}", uri, e);
    }
}

/// Pure predicate: returns the URI of the cell at `(row, col)` when
/// a `Press + Left + modifier_held` event arrives on a linked cell;
/// otherwise `None`. Centralizes the interception decision so
/// `dispatch_mouse_buttons` only has to check the return value.
pub(crate) fn should_open_at(
    grid: &bevy_terminal_renderer::schema::TerminalGrid,
    row: u16,
    col: u16,
    button: bevy_terminal::MouseButtonKind,
    kind: bevy_terminal::ButtonEventKind,
    modifier_held: bool,
) -> Option<bevy_terminal_renderer::schema::HyperlinkUri> {
    if !modifier_held || button != bevy_terminal::MouseButtonKind::Left {
        return None;
    }
    if !matches!(kind, bevy_terminal::ButtonEventKind::Press) {
        return None;
    }
    grid.hyperlink_at(row, col).map(|(_id, uri)| uri.clone())
}

const ALLOWED_SCHEMES: &[&str] = &["http", "https", "mailto", "ftp"];

/// Parses an RFC 3986 scheme: first byte ALPHA, continuation
/// ALPHA / DIGIT / `+` / `-` / `.`. Returns `None` for malformed input.
fn scheme_of(uri: &str) -> Option<&str> {
    let (scheme, _) = uri.split_once(':')?;
    let mut bytes = scheme.bytes();
    let first = bytes.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.') {
        return None;
    }
    Some(scheme)
}

/// Returns `true` when `uri` carries a scheme on the v1 allowlist
/// (`http`, `https`, `mailto`, `ftp`), case-insensitive.
fn is_allowed(uri: &str) -> bool {
    scheme_of(uri)
        .map(|s| s.to_ascii_lowercase())
        .is_some_and(|s| ALLOWED_SCHEMES.contains(&s.as_str()))
}

fn hyperlink_hover_and_cursor(
    mut hover: ResMut<HyperlinkHoverState>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    hosts: Query<(Entity, &ComputedNode, &UiGlobalTransform), With<ActivityHostNode>>,
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

    let mods = current_modifiers(&keys);
    hover.modifier_held = link_modifier_held(&mods);

    let Some((entity, local)) = resolve_pane_at_phys(&hosts, cursor_phys) else {
        clear_target(&mut hover, &mut cursor_icons);
        return;
    };
    let Ok(grid) = grids.get(entity) else {
        clear_target(&mut hover, &mut cursor_icons);
        return;
    };
    let (col, row, _side) = cell_at_local(local, cell_w_phys, cell_h_phys, grid.cols, grid.rows);
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

/// Resets every field of the hover state and forces the I-beam cursor.
/// Used when the window or cursor is unobservable — modifier state
/// cannot be trusted because the system bailed before reading keys.
fn clear_hover(
    hover: &mut HyperlinkHoverState,
    cursor_icons: &mut Query<&mut CursorIcon, With<PrimaryWindow>>,
) {
    hover.entity = None;
    hover.hyperlink_id = None;
    hover.modifier_held = false;
    write_cursor_icon(cursor_icons, SystemCursorIcon::Text);
}

/// Resets only the per-cursor fields (entity + hyperlink_id) and the
/// cursor icon, preserving `modifier_held` set earlier this frame. Used
/// when the cursor is observable but lands outside any pane or the
/// hovered entity has no `TerminalGrid` — modifier state remains valid.
fn clear_target(
    hover: &mut HyperlinkHoverState,
    cursor_icons: &mut Query<&mut CursorIcon, With<PrimaryWindow>>,
) {
    hover.entity = None;
    hover.hyperlink_id = None;
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

    use bevy::prelude::Color;
    use bevy_terminal::{ButtonEventKind, MouseButtonKind};
    use bevy_terminal_renderer::schema::{Cell, HyperlinkId, HyperlinkUri};

    #[test]
    fn scheme_of_rejects_leading_digit() {
        assert_eq!(scheme_of("1abc:foo"), None);
    }

    #[test]
    fn scheme_of_rejects_empty_scheme() {
        assert_eq!(scheme_of(":foo"), None);
        assert_eq!(scheme_of("no-colon"), None);
    }

    #[test]
    fn scheme_of_rejects_disallowed_punctuation() {
        assert_eq!(scheme_of("my_scheme:foo"), None);
    }

    #[test]
    fn scheme_of_accepts_canonical_schemes() {
        assert_eq!(scheme_of("http:foo"), Some("http"));
        assert_eq!(scheme_of("https:foo"), Some("https"));
        assert_eq!(scheme_of("git+ssh:foo"), Some("git+ssh"));
        assert_eq!(scheme_of("a-b.c:foo"), Some("a-b.c"));
    }

    #[test]
    fn is_allowed_accepts_canonical_schemes_case_insensitive() {
        assert!(is_allowed("http://example.com"));
        assert!(is_allowed("HTTPS://example.com"));
        assert!(is_allowed("Mailto:foo@example"));
        assert!(is_allowed("ftp://example.com"));
    }

    #[test]
    fn is_allowed_rejects_dangerous_or_unknown_schemes() {
        assert!(!is_allowed("javascript:alert(1)"));
        assert!(!is_allowed("file:///etc/passwd"));
        assert!(!is_allowed("data:text/html,<script>"));
        assert!(!is_allowed("vscode://"));
        assert!(!is_allowed(""));
        assert!(!is_allowed("no-colon-here"));
    }

    fn make_grid_with_link(
        row: usize,
        col: usize,
        id: HyperlinkId,
    ) -> bevy_terminal_renderer::schema::TerminalGrid {
        let cell = Cell {
            text: "x".to_string(),
            width: 1,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: Some(id),
        };
        let mut cells = vec![
            vec![
                Cell {
                    text: " ".to_string(),
                    width: 1,
                    fg: Color::WHITE,
                    bg: Color::BLACK,
                    style: 0,
                    hyperlink_id: None,
                };
                col + 1
            ];
            row + 1
        ];
        cells[row][col] = cell;
        bevy_terminal_renderer::schema::TerminalGrid {
            cols: (col as u16) + 1,
            rows: (row as u16) + 1,
            cells,
            hyperlinks: vec![(id, HyperlinkUri::new("https://example.com"))],
            ..Default::default()
        }
    }

    #[test]
    fn should_open_at_returns_none_without_modifier() {
        let grid = make_grid_with_link(0, 0, HyperlinkId(1));
        let result = should_open_at(
            &grid,
            0,
            0,
            MouseButtonKind::Left,
            ButtonEventKind::Press,
            false,
        );
        assert!(result.is_none());
    }

    #[test]
    fn should_open_at_returns_none_for_non_left_button() {
        let grid = make_grid_with_link(0, 0, HyperlinkId(1));
        for button in [MouseButtonKind::Middle, MouseButtonKind::Right] {
            let result = should_open_at(&grid, 0, 0, button, ButtonEventKind::Press, true);
            assert!(result.is_none(), "button={:?}", button);
        }
    }

    #[test]
    fn should_open_at_returns_none_for_release_event() {
        let grid = make_grid_with_link(0, 0, HyperlinkId(1));
        let result = should_open_at(
            &grid,
            0,
            0,
            MouseButtonKind::Left,
            ButtonEventKind::Release,
            true,
        );
        assert!(result.is_none());
    }

    #[test]
    fn should_open_at_returns_uri_for_press_left_modifier_on_link() {
        let grid = make_grid_with_link(0, 0, HyperlinkId(1));
        let uri = should_open_at(
            &grid,
            0,
            0,
            MouseButtonKind::Left,
            ButtonEventKind::Press,
            true,
        )
        .expect("hyperlink present");
        assert_eq!(uri.as_str(), "https://example.com");
    }
}
