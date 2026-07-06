//! OSC 8 hyperlink hover detection and cursor-icon control — the single
//! authority for `HyperlinkHoverState` (the renderer underline) and the window
//! `CursorIcon` over every terminal surface: tmux panes, the Default-mode shell,
//! and webview hosts (all are `OrzmaTerminal` entities). Over a linked cell the
//! cursor becomes a pointer while the platform activation modifier
//! (`link_modifier_held`) is held, and text otherwise; over a webview host the
//! cursor is left to `bevy_cef`'s `SystemCursorIconPlugin`. Surfaces with input
//! suppressed (`MouseDisabled`: vi mode, IME, focused webview, unfocused
//! window) are skipped, so hover never advertises a link the mouse dispatcher
//! would refuse to open. Hyperlink activation (Cmd/Ctrl-click → `OpenUri`) now lives in
//! `crate::input::mouse` (deciders `decide_button`/`resolve_button_event`);
//! `crate::action::terminal` keeps only the apply observer
//! (`on_terminal_open_uri` → `try_open_uri`).

use crate::input::focus::MouseDisabled;
use crate::input::{InputPhase, current_modifiers};
use crate::surface::OrzmaTerminal;
use crate::surface::geometry::topmost_surface_at;
use crate::surface::geometry::{cell_at_local, phys_to_pane_local};
use bevy::ecs::entity::Entity;
use bevy::input::ButtonInput;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorIcon, CursorMoved, PrimaryWindow, SystemCursorIcon, Window};
use bevy_cef::prelude::WebviewSource;
use orzma_configs::shortcuts::Modifiers;
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzma_tty_renderer::schema::{HyperlinkHoverState, TerminalGrid};

/// Plugin: registers `hyperlink_hover_and_cursor` in `InputPhase::Hover`.
pub(crate) struct HyperlinkInputPlugin;

impl Plugin for HyperlinkInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            hyperlink_hover_and_cursor
                .run_if(
                    on_message::<MouseMotion>
                        .or(on_message::<CursorMoved>)
                        .or(on_message::<KeyboardInput>),
                )
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

fn hyperlink_hover_and_cursor(
    mut hover: ResMut<HyperlinkHoverState>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    surfaces: Query<
        (Entity, &ComputedNode, &UiGlobalTransform),
        (With<OrzmaTerminal>, Without<MouseDisabled>),
    >,
    grids: Query<&TerminalGrid>,
    webview_hosts: Query<&WebviewSource>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Ok(window) = windows.single() else {
        reset_hover_state(&mut hover);
        apply_cursor(&mut cursor_icons, cursor_decision(HoverTarget::Default));
        return;
    };
    let scale = window.scale_factor();
    let Some(cursor_logical) = window.cursor_position() else {
        reset_hover_state(&mut hover);
        apply_cursor(&mut cursor_icons, cursor_decision(HoverTarget::Default));
        return;
    };
    let cursor_phys = cursor_logical * scale;
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);

    let mods = current_modifiers(&keys);
    hover.modifier_held = link_modifier_held(&mods);

    hover.entity = None;
    hover.hyperlink_id = None;
    let target = match topmost_surface_at(cursor_phys, surfaces.iter()) {
        None => HoverTarget::Default,
        Some(entity) => {
            if webview_hosts.contains(entity) {
                HoverTarget::Webview
            } else if let Ok(grid) = grids.get(entity) {
                let id = surfaces
                    .get(entity)
                    .ok()
                    .and_then(|(_, node, transform)| {
                        phys_to_pane_local(node, transform, cursor_phys)
                    })
                    .map(|local| {
                        cell_at_local(local, cell_w_phys, cell_h_phys, grid.cols, grid.rows)
                    })
                    .and_then(|(col, row, _side)| {
                        grid.hyperlink_at(
                            row.saturating_sub(1) as u16,
                            col.saturating_sub(1) as u16,
                        )
                    })
                    .map(|(id, _uri)| id);
                hover.entity = Some(entity);
                hover.hyperlink_id = id;
                HoverTarget::Terminal {
                    has_link: id.is_some(),
                    modifier_held: hover.modifier_held,
                }
            } else {
                HoverTarget::Default
            }
        }
    };

    apply_cursor(&mut cursor_icons, cursor_decision(target));
}

/// Clears every per-cursor field of the hover state, including
/// `modifier_held`. Used on the early-return paths where the keyboard
/// was not read, so the modifier state cannot be trusted.
fn reset_hover_state(hover: &mut HyperlinkHoverState) {
    hover.entity = None;
    hover.hyperlink_id = None;
    hover.modifier_held = false;
}

/// Applies a cursor decision: writes the icon when `Some`, leaves the
/// cursor untouched (CEF-owned) when `None`.
fn apply_cursor(
    cursor_icons: &mut Query<&mut CursorIcon, With<PrimaryWindow>>,
    decision: Option<SystemCursorIcon>,
) {
    if let Some(icon) = decision {
        write_cursor_icon(cursor_icons, icon);
    }
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

/// Which region the mouse is over, distilled to what the cursor needs.
/// `Default` covers everything that is neither terminal grid nor a CEF
/// render area (chrome, gaps, an unobservable window).
enum HoverTarget {
    Terminal { has_link: bool, modifier_held: bool },
    Webview,
    Default,
}

/// Maps a `HoverTarget` to the cursor to set. `None` means "leave the
/// cursor untouched" so `bevy_cef`'s `SystemCursorIconPlugin` owns it
/// over CEF render areas.
fn cursor_decision(target: HoverTarget) -> Option<SystemCursorIcon> {
    match target {
        HoverTarget::Terminal {
            has_link: true,
            modifier_held: true,
        } => Some(SystemCursorIcon::Pointer),
        HoverTarget::Terminal { .. } => Some(SystemCursorIcon::Text),
        HoverTarget::Webview => None,
        HoverTarget::Default => Some(SystemCursorIcon::Default),
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

    #[test]
    fn cursor_decision_default_is_arrow() {
        assert_eq!(
            cursor_decision(HoverTarget::Default),
            Some(SystemCursorIcon::Default)
        );
    }

    #[test]
    fn cursor_decision_webview_leaves_cursor_alone() {
        assert_eq!(cursor_decision(HoverTarget::Webview), None);
    }

    #[test]
    fn cursor_decision_terminal_link_with_modifier_is_pointer() {
        assert_eq!(
            cursor_decision(HoverTarget::Terminal {
                has_link: true,
                modifier_held: true,
            }),
            Some(SystemCursorIcon::Pointer)
        );
    }

    #[test]
    fn cursor_decision_terminal_link_without_modifier_is_text() {
        assert_eq!(
            cursor_decision(HoverTarget::Terminal {
                has_link: true,
                modifier_held: false,
            }),
            Some(SystemCursorIcon::Text)
        );
    }

    #[test]
    fn cursor_decision_terminal_no_link_is_text() {
        assert_eq!(
            cursor_decision(HoverTarget::Terminal {
                has_link: false,
                modifier_held: true,
            }),
            Some(SystemCursorIcon::Text)
        );
    }

    #[test]
    fn cursor_decision_terminal_plain_is_text() {
        assert_eq!(
            cursor_decision(HoverTarget::Terminal {
                has_link: false,
                modifier_held: false,
            }),
            Some(SystemCursorIcon::Text)
        );
    }

    use orzma_tty_renderer::CellMetrics;

    fn hover_test_metrics() -> TerminalCellMetricsResource {
        TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 16,
        }
    }

    #[test]
    fn hover_with_no_panes_leaves_entity_none_and_cursor_default() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseMotion>();
        app.init_resource::<HyperlinkHoverState>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(hover_test_metrics());
        app.add_systems(Update, hyperlink_hover_and_cursor);
        let mut window = Window::default();
        window.set_cursor_position(Some(Vec2::new(10.0, 10.0)));
        let window_entity = app
            .world_mut()
            .spawn((
                window,
                PrimaryWindow,
                CursorIcon::System(SystemCursorIcon::Pointer),
            ))
            .id();
        app.world_mut().resource_mut::<HyperlinkHoverState>().entity = Some(window_entity);
        app.update();
        let hover = app.world().resource::<HyperlinkHoverState>();
        assert_eq!(hover.entity, None);
        assert_eq!(hover.hyperlink_id, None);
        let icon = app.world().entity(window_entity).get::<CursorIcon>();
        assert_eq!(
            icon,
            Some(&CursorIcon::System(SystemCursorIcon::Default)),
            "with no pane under the cursor the decision is Default"
        );
    }

    #[test]
    fn hover_over_default_mode_terminal_link_sets_state_and_pointer() {
        use orzma_tty_renderer::schema::{Cell, HyperlinkId, HyperlinkUri};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseMotion>();
        app.init_resource::<HyperlinkHoverState>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(hover_test_metrics());
        app.add_systems(Update, hyperlink_hover_and_cursor);

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            if cfg!(target_os = "macos") {
                keys.press(KeyCode::SuperLeft);
            } else {
                keys.press(KeyCode::ControlLeft);
            }
        }

        let mut window = Window::default();
        window.set_cursor_position(Some(Vec2::new(4.0, 8.0)));
        let window_entity = app
            .world_mut()
            .spawn((
                window,
                PrimaryWindow,
                CursorIcon::System(SystemCursorIcon::Default),
            ))
            .id();

        let grid = TerminalGrid {
            cols: 10,
            rows: 5,
            cells: vec![vec![Cell {
                text: "x".to_string(),
                width: 1,
                fg: Color::WHITE,
                bg: Color::BLACK,
                style: 0,
                hyperlink_id: Some(HyperlinkId(7)),
            }]],
            hyperlinks: vec![(HyperlinkId(7), HyperlinkUri::new("https://example.com"))],
            ..default()
        };
        let term = app
            .world_mut()
            .spawn((
                OrzmaTerminal,
                ComputedNode {
                    size: Vec2::new(80.0, 80.0),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(40.0, 40.0),
                grid,
            ))
            .id();

        app.update();

        let hover = app.world().resource::<HyperlinkHoverState>();
        assert_eq!(
            hover.entity,
            Some(term),
            "hover must resolve to the Default-mode OrzmaTerminal (no TmuxPane) under the cursor"
        );
        assert_eq!(
            hover.hyperlink_id,
            Some(HyperlinkId(7)),
            "the linked cell's hyperlink id must populate the hover state"
        );
        assert!(hover.modifier_held, "the link-activation modifier is held");
        let icon = app.world().entity(window_entity).get::<CursorIcon>();
        assert_eq!(
            icon,
            Some(&CursorIcon::System(SystemCursorIcon::Pointer)),
            "a link under the cursor with the modifier held shows the pointer"
        );
    }

    #[test]
    fn hover_skips_mouse_disabled_surface() {
        use orzma_tty_renderer::schema::{Cell, HyperlinkId, HyperlinkUri};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseMotion>();
        app.init_resource::<HyperlinkHoverState>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(hover_test_metrics());
        app.add_systems(Update, hyperlink_hover_and_cursor);

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            if cfg!(target_os = "macos") {
                keys.press(KeyCode::SuperLeft);
            } else {
                keys.press(KeyCode::ControlLeft);
            }
        }

        let mut window = Window::default();
        window.set_cursor_position(Some(Vec2::new(4.0, 8.0)));
        let window_entity = app
            .world_mut()
            .spawn((
                window,
                PrimaryWindow,
                CursorIcon::System(SystemCursorIcon::Default),
            ))
            .id();

        let grid = TerminalGrid {
            cols: 10,
            rows: 5,
            cells: vec![vec![Cell {
                text: "x".to_string(),
                width: 1,
                fg: Color::WHITE,
                bg: Color::BLACK,
                style: 0,
                hyperlink_id: Some(HyperlinkId(7)),
            }]],
            hyperlinks: vec![(HyperlinkId(7), HyperlinkUri::new("https://example.com"))],
            ..default()
        };
        app.world_mut().spawn((
            OrzmaTerminal,
            MouseDisabled,
            ComputedNode {
                size: Vec2::new(80.0, 80.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(40.0, 40.0),
            grid,
        ));

        app.update();

        let hover = app.world().resource::<HyperlinkHoverState>();
        assert_eq!(
            hover.entity, None,
            "a MouseDisabled surface must not be hovered — the click is suppressed, so no link affordance"
        );
        assert_eq!(hover.hyperlink_id, None);
        let icon = app.world().entity(window_entity).get::<CursorIcon>();
        assert_eq!(
            icon,
            Some(&CursorIcon::System(SystemCursorIcon::Default)),
            "with input suppressed the cursor stays the arrow, not a link pointer"
        );
    }

    #[test]
    fn hover_over_webview_host_leaves_cursor_to_cef() {
        use orzma_tty_renderer::schema::{Cell, HyperlinkId, HyperlinkUri};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseMotion>();
        app.init_resource::<HyperlinkHoverState>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(hover_test_metrics());
        app.add_systems(Update, hyperlink_hover_and_cursor);

        let mut window = Window::default();
        window.set_cursor_position(Some(Vec2::new(4.0, 8.0)));
        // A distinctive starting cursor that the Webview decision must leave untouched.
        let window_entity = app
            .world_mut()
            .spawn((
                window,
                PrimaryWindow,
                CursorIcon::System(SystemCursorIcon::Pointer),
            ))
            .id();

        // A webview host: an OrzmaTerminal carrying WebviewSource. `on_add_inject_render`
        // would also give it a (rendered-over) grid, so the webview check must win.
        let grid = TerminalGrid {
            cols: 10,
            rows: 5,
            cells: vec![vec![Cell {
                text: "x".to_string(),
                width: 1,
                fg: Color::WHITE,
                bg: Color::BLACK,
                style: 0,
                hyperlink_id: Some(HyperlinkId(7)),
            }]],
            hyperlinks: vec![(HyperlinkId(7), HyperlinkUri::new("https://example.com"))],
            ..default()
        };
        app.world_mut().spawn((
            OrzmaTerminal,
            WebviewSource::new("orzma://example/index.html"),
            ComputedNode {
                size: Vec2::new(80.0, 80.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(40.0, 40.0),
            grid,
        ));

        app.update();

        let hover = app.world().resource::<HyperlinkHoverState>();
        assert_eq!(
            hover.entity, None,
            "a webview host must not be treated as a terminal even though it carries a grid"
        );
        let icon = app.world().entity(window_entity).get::<CursorIcon>();
        assert_eq!(
            icon,
            Some(&CursorIcon::System(SystemCursorIcon::Pointer)),
            "over a webview host the cursor is left untouched for bevy_cef to own"
        );
    }
}
