//! OSC 8 hyperlink hover detection and cursor-icon control across every
//! terminal surface (the shell terminal and webview hosts): the only
//! writer of `HyperlinkHoverState` and the window's `CursorIcon`.

use crate::input::focus::{MouseClaimedByWebview, MouseDisabled};
use crate::input::mouse::separator::{GrabbedSeparator, SeparatorHit, SeparatorNodes};
use crate::input::{InputPhase, current_modifiers};
use crate::surface::OrzmaTerminal;
use crate::surface::geometry::topmost_surface_at;
use crate::surface::geometry::{cell_at_local, cell_pitch_phys, phys_to_pane_local};
use bevy::ecs::entity::Entity;
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonInput;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, ComputedStackIndex, UiGlobalTransform};
use bevy::window::{CursorIcon, CursorMoved, PrimaryWindow, SystemCursorIcon, Window};
use bevy_cef::prelude::WebviewSource;
use bevy_orzma_tty_renderer::TerminalCellMetricsResource;
use bevy_orzma_tty_renderer::schema::{HyperlinkHoverState, TerminalCells, TerminalView};
use bevy_orzmux::prelude::{OrzmuxSeparator, PaneGeometry, SplitOrientation};
use orzma_configs::shortcuts::Modifiers;

/// Adds hyperlink hover detection and cursor-icon control for every
/// terminal surface.
pub(super) struct HyperlinkInputPlugin;

impl Plugin for HyperlinkInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, insert_initial_cursor_icon)
            .add_systems(
                Update,
                hyperlink_hover_and_cursor
                    .run_if(
                        on_message::<MouseMotion>
                            .or_else(on_message::<CursorMoved>)
                            .or_else(on_message::<KeyboardInput>),
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

/// Every mouse-enabled `OrzmaTerminal` surface.
type HoverSurfaces<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ComputedNode,
        &'static ComputedStackIndex,
        &'static UiGlobalTransform,
    ),
    (
        With<OrzmaTerminal>,
        Without<MouseDisabled>,
        Without<MouseClaimedByWebview>,
    ),
>;

/// Skips any surface with input suppressed (`MouseDisabled`) or claimed by a
/// webview (`MouseClaimedByWebview`), so hover never advertises a link the
/// mouse dispatcher would refuse to open. A divider the pointer holds or
/// hovers claims the cursor before any surface is read, leaving the hover
/// state empty; a held divider keeps the cursor even while the pointer
/// reports no position, which is what a drag past the window's edge does.
fn hyperlink_hover_and_cursor(
    mut hover: ResMut<HyperlinkHoverState>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    targets: HoverTargetParams,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Some(cursor_phys) = windows
        .single()
        .ok()
        .and_then(|window| Some(window.cursor_position()? * window.scale_factor()))
    else {
        reset_hover_state(&mut hover);
        apply_cursor(&mut cursor_icons, cursor_decision(targets.unlocated()));
        return;
    };

    let mods = current_modifiers(&keys);
    hover.modifier_held = link_modifier_held(&mods);

    hover.entity = None;
    hover.hyperlink_id = None;

    let target = targets.target(&mut hover, cursor_phys);
    apply_cursor(&mut cursor_icons, cursor_decision(target));
}

/// Clears every per-cursor field of the hover state, including
/// `modifier_held`. Call this when the keyboard was not read this frame,
/// since the modifier state cannot be trusted otherwise.
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
    Separator(SplitOrientation),
    Terminal { has_link: bool, modifier_held: bool },
    Webview,
    Default,
}

/// What the hover decision reads to tell which region the pointer is
/// over.
#[derive(SystemParam)]
struct HoverTargetParams<'w, 's> {
    surfaces: HoverSurfaces<'w, 's>,
    terminals: Query<'w, 's, (&'static TerminalView, &'static TerminalCells)>,
    webview_hosts: Query<'w, 's, &'static WebviewSource>,
    separators: SeparatorNodes<'w, 's>,
    grabbed: Query<'w, 's, &'static OrzmuxSeparator, With<GrabbedSeparator>>,
    metrics: Res<'w, TerminalCellMetricsResource>,
    geometry: Option<Res<'w, PaneGeometry>>,
}

impl HoverTargetParams<'_, '_> {
    /// The region under `cursor_phys`, in window physical px: the divider
    /// a drag holds, else the divider whose grab band contains the
    /// pointer, else the surface beneath it. When a divider claims the
    /// pointer, no surface is read and `hover` is left untouched.
    fn target(&self, hover: &mut HyperlinkHoverState, cursor_phys: Vec2) -> HoverTarget {
        match self.held().or_else(|| self.hovered(cursor_phys)) {
            Some(orientation) => HoverTarget::Separator(orientation),
            None => self.over_surface(hover, cursor_phys),
        }
    }

    /// The region for a pointer whose position is unknown: the divider a
    /// drag holds, else `Default`.
    fn unlocated(&self) -> HoverTarget {
        self.held()
            .map_or(HoverTarget::Default, HoverTarget::Separator)
    }

    /// The orientation of the divider a drag holds.
    fn held(&self) -> Option<SplitOrientation> {
        self.grabbed
            .iter()
            .next()
            .map(|separator| separator.orientation)
    }

    /// The orientation of the divider whose grab band contains
    /// `cursor_phys`, in window physical px, or `None` while the pane
    /// geometry is unknown.
    fn hovered(&self, cursor_phys: Vec2) -> Option<SplitOrientation> {
        let geometry = self.geometry.as_deref()?;
        SeparatorHit::at(cursor_phys, geometry, self.separators.iter()).map(|hit| hit.orientation)
    }

    /// The region for the topmost mouse-enabled surface under
    /// `cursor_phys`, in window physical px. Records that surface and
    /// the hyperlink id of the cell under the pointer in `hover`,
    /// leaving both untouched over anything but a terminal grid.
    fn over_surface(&self, hover: &mut HyperlinkHoverState, cursor_phys: Vec2) -> HoverTarget {
        let Some(entity) = topmost_surface_at(cursor_phys, self.surfaces.iter()) else {
            return HoverTarget::Default;
        };
        if self.webview_hosts.contains(entity) {
            return HoverTarget::Webview;
        }
        let Ok((view, cells)) = self.terminals.get(entity) else {
            return HoverTarget::Default;
        };
        let (cell_w, cell_h) = cell_pitch_phys(&self.metrics.metrics);
        let id = self
            .surfaces
            .get(entity)
            .ok()
            .and_then(|(_, node, _, transform)| phys_to_pane_local(node, transform, cursor_phys))
            .map(|local| cell_at_local(local, cell_w, cell_h, view.cols, view.rows))
            .and_then(|(col, row, _side)| {
                cells.hyperlink_at(row.saturating_sub(1) as u16, col.saturating_sub(1) as u16)
            })
            .map(|(id, _uri)| id);
        hover.entity = Some(entity);
        hover.hyperlink_id = id;
        HoverTarget::Terminal {
            has_link: id.is_some(),
            modifier_held: hover.modifier_held,
        }
    }
}

/// Maps a `HoverTarget` to the cursor to set. `None` means "leave the
/// cursor untouched" so `bevy_cef`'s `SystemCursorIconPlugin` owns it
/// over CEF render areas.
fn cursor_decision(target: HoverTarget) -> Option<SystemCursorIcon> {
    match target {
        HoverTarget::Separator(SplitOrientation::Vertical) => Some(SystemCursorIcon::ColResize),
        HoverTarget::Separator(SplitOrientation::Horizontal) => Some(SystemCursorIcon::RowResize),
        HoverTarget::Terminal {
            has_link: true,
            modifier_held: true,
        } => Some(SystemCursorIcon::Pointer),
        HoverTarget::Terminal { .. } => Some(SystemCursorIcon::Text),
        HoverTarget::Webview => None,
        HoverTarget::Default => Some(SystemCursorIcon::Default),
    }
}

/// Inserts an initial `CursorIcon::System(SystemCursorIcon::Default)`
/// (the arrow) on the primary window so the hover system can mutate the
/// component without first having to insert it. The arrow is the default
/// for non-terminal regions; the hover system narrows it to the I-beam
/// over terminal text.
fn insert_initial_cursor_icon(
    mut commands: Commands,
    windows: Query<Entity, (With<PrimaryWindow>, Without<CursorIcon>)>,
) {
    for window in windows.iter() {
        commands
            .entity(window)
            .insert(CursorIcon::System(SystemCursorIcon::Default));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_orzma_tty_renderer::schema::HyperlinkId;
    use bevy_orzmux::prelude::SplitId;

    fn empty() -> Modifiers {
        Modifiers::default()
    }

    fn id(value: u32) -> HyperlinkId {
        HyperlinkId::new(value).expect("nonzero")
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

    /// Asserts that a hovered divider asks for the resize cursor
    /// matching its direction.
    ///
    /// Case: the user moves the pointer over a column divider, then over
    /// a row divider.
    #[test]
    fn a_hovered_divider_asks_for_the_matching_resize_cursor() {
        assert_eq!(
            cursor_decision(HoverTarget::Separator(SplitOrientation::Vertical)),
            Some(SystemCursorIcon::ColResize)
        );
        assert_eq!(
            cursor_decision(HoverTarget::Separator(SplitOrientation::Horizontal)),
            Some(SystemCursorIcon::RowResize)
        );
    }

    use bevy_orzma_tty_renderer::CellMetrics;

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

    /// A 10x5 grid whose top-left cell links to `https://example.com` as
    /// `HyperlinkId::new(7)`, shared by the hover tests.
    fn linked_grid() -> (TerminalView, TerminalCells) {
        use bevy_orzma_tty_renderer::schema::{Color, GridCell, GridSlot, HyperlinkUri};
        use std::collections::HashMap;
        let mut rows = vec![vec![GridSlot::Empty; 10]; 5];
        rows[0][0] = GridSlot::Cell(GridCell {
            text: "x".to_string(),
            fg: Color::DefaultForeground,
            bg: Color::DefaultBackground,
            style: 0,
            hyperlink: Some(id(7)),
        });
        (
            TerminalView {
                cols: 10,
                rows: 5,
                ..default()
            },
            TerminalCells {
                cells: rows,
                hyperlinks: HashMap::from([(id(7), HyperlinkUri::new("https://example.com"))]),
                ..default()
            },
        )
    }

    /// Asserts that hovering a linked cell with the activation modifier held
    /// records the surface and hyperlink id in the hover state and switches
    /// the window cursor to a pointer.
    ///
    /// Case: the user holds Cmd (Ctrl off macOS) and moves the mouse over an
    /// OSC 8 hyperlink in the terminal.
    #[test]
    fn hover_over_terminal_link_sets_state_and_pointer() {
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

        let (view, cells) = linked_grid();
        let term = app
            .world_mut()
            .spawn((
                OrzmaTerminal,
                ComputedNode {
                    size: Vec2::new(80.0, 80.0),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(40.0, 40.0),
                view,
                cells,
            ))
            .id();

        app.update();

        let hover = app.world().resource::<HyperlinkHoverState>();
        assert_eq!(
            hover.entity,
            Some(term),
            "hover must resolve to the OrzmaTerminal under the cursor"
        );
        assert_eq!(
            hover.hyperlink_id,
            Some(id(7)),
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

    /// Asserts that a `MouseDisabled` surface is never hovered: the hover
    /// state stays empty and the cursor keeps the default arrow even over a
    /// linked cell.
    ///
    /// Case: the pointer crosses a hyperlink on a terminal whose mouse input
    /// is suppressed, such as one in vi mode.
    #[test]
    fn hover_skips_mouse_disabled_surface() {
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

        let (view, cells) = linked_grid();
        app.world_mut().spawn((
            OrzmaTerminal,
            MouseDisabled,
            ComputedNode {
                size: Vec2::new(80.0, 80.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(40.0, 40.0),
            view,
            cells,
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

    /// Asserts that a `MouseClaimedByWebview` surface is never hovered: the
    /// hover state stays empty and the cursor keeps the default arrow even
    /// over a linked cell.
    ///
    /// Case: the pointer crosses a hyperlink drawn underneath a mounted
    /// page, where the click belongs to the page rather than the terminal.
    #[test]
    fn hover_skips_webview_claimed_surface() {
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
        app.world_mut().spawn((
            window,
            PrimaryWindow,
            CursorIcon::System(SystemCursorIcon::Default),
        ));

        let (view, cells) = linked_grid();
        app.world_mut().spawn((
            OrzmaTerminal,
            MouseClaimedByWebview,
            ComputedNode {
                size: Vec2::new(80.0, 80.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(40.0, 40.0),
            view,
            cells,
        ));

        app.update();

        let hover = app.world().resource::<HyperlinkHoverState>();
        assert_eq!(
            hover.entity, None,
            "a claimed surface must not be hovered — the click belongs to the page, so no link affordance"
        );
        assert_eq!(hover.hyperlink_id, None);
    }

    /// Asserts that a surface hosting a webview is not treated as a
    /// terminal: the hover state stays empty and the window cursor is left
    /// untouched for CEF to own.
    ///
    /// Case: the pointer moves over an inline webview overlay whose page
    /// manages its own cursor.
    #[test]
    fn hover_over_webview_host_leaves_cursor_to_cef() {
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
        let (view, cells) = linked_grid();
        app.world_mut().spawn((
            OrzmaTerminal,
            WebviewSource::new("orzma://example/index.html"),
            ComputedNode {
                size: Vec2::new(80.0, 80.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(40.0, 40.0),
            view,
            cells,
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

    /// A hover world at `scale`, with the pointer at `cursor_phys` in
    /// window physical px, an 8x16 physical px cell `PaneGeometry`, and
    /// one linked terminal surface filling the top-left 160x160
    /// physical px.
    fn divider_hover_app(scale: f32, cursor_phys: Vec2) -> App {
        use bevy::math::DVec2;
        use bevy::window::WindowResolution;
        use orzma_tty::CellPixels;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .init_resource::<HyperlinkHoverState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(hover_test_metrics())
            .insert_resource(PaneGeometry {
                cell_px: CellPixels {
                    width: 8,
                    height: 16,
                },
                scale_factor: scale,
            })
            .add_systems(Update, hyperlink_hover_and_cursor);

        let mut window = Window {
            resolution: WindowResolution::new(800, 400).with_scale_factor_override(scale),
            ..default()
        };
        window.set_physical_cursor_position(Some(DVec2::new(
            f64::from(cursor_phys.x),
            f64::from(cursor_phys.y),
        )));
        app.world_mut().spawn((
            window,
            PrimaryWindow,
            CursorIcon::System(SystemCursorIcon::Default),
        ));

        app.world_mut().spawn((
            OrzmaTerminal,
            ComputedNode {
                size: Vec2::splat(160.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(80.0, 80.0),
            linked_grid(),
        ));
        app
    }

    /// The primary window's cursor icon.
    fn window_cursor(app: &mut App) -> Option<CursorIcon> {
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .ok()?;
        app.world().entity(window).get::<CursorIcon>().cloned()
    }

    /// Asserts that a divider whose grab band contains the pointer takes
    /// the cursor from the pane beneath it and leaves the hyperlink
    /// hover state empty.
    ///
    /// Case: on a Retina display the user slides the pointer onto the
    /// groove between two side-by-side panes, stopping a few physical px
    /// off the painted line.
    #[test]
    fn a_divider_under_the_pointer_takes_the_cursor_from_the_pane() {
        let mut app = divider_hover_app(2.0, Vec2::new(20.0, 40.0));
        app.world_mut().spawn((
            OrzmuxSeparator {
                split: SplitId(1),
                orientation: SplitOrientation::Vertical,
            },
            ComputedNode {
                size: Vec2::new(2.0, 160.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(26.0, 80.0),
        ));

        app.update();

        assert_eq!(
            window_cursor(&mut app),
            Some(CursorIcon::System(SystemCursorIcon::ColResize)),
            "the divider owns the pointer, so the column-resize cursor wins over the pane's I-beam"
        );
        assert_eq!(
            app.world().resource::<HyperlinkHoverState>().entity,
            None,
            "a pointer the divider owns hovers no terminal, so no link affordance is offered"
        );
    }

    /// Asserts that a drag in flight keeps its resize cursor on a frame
    /// the window reports no pointer position at all.
    ///
    /// Case: the user drags a row divider past the window's bottom edge,
    /// so the pointer leaves the client area while the button is held.
    #[test]
    fn a_held_drag_keeps_the_resize_cursor_off_the_window() {
        let mut app = divider_hover_app(2.0, Vec2::new(20.0, 4000.0));
        app.world_mut().spawn((
            OrzmuxSeparator {
                split: SplitId(1),
                orientation: SplitOrientation::Horizontal,
            },
            GrabbedSeparator::held(SplitId(1), SplitOrientation::Horizontal),
            ComputedNode {
                size: Vec2::new(160.0, 2.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(80.0, 160.0),
        ));

        app.update();

        assert_eq!(
            window_cursor(&mut app),
            Some(CursorIcon::System(SystemCursorIcon::RowResize)),
            "an unreported pointer does not end the drag, so the arrow must not come back"
        );
    }

    /// Asserts that a drag in flight holds the resize cursor for its own
    /// divider while the pointer sits over a pane outside every grab
    /// band.
    ///
    /// Case: the user presses on a row divider and drags well up into
    /// the pane above it.
    #[test]
    fn a_held_drag_keeps_the_resize_cursor_over_the_pane() {
        let mut app = divider_hover_app(2.0, Vec2::new(20.0, 40.0));
        app.world_mut().spawn((
            OrzmuxSeparator {
                split: SplitId(1),
                orientation: SplitOrientation::Horizontal,
            },
            GrabbedSeparator::held(SplitId(1), SplitOrientation::Horizontal),
            ComputedNode {
                size: Vec2::new(160.0, 2.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(80.0, 400.0),
        ));

        app.update();

        assert_eq!(
            window_cursor(&mut app),
            Some(CursorIcon::System(SystemCursorIcon::RowResize)),
            "the held divider decides the cursor wherever the pointer travels"
        );
        assert_eq!(
            app.world().resource::<HyperlinkHoverState>().entity,
            None,
            "a drag in flight hovers no terminal"
        );
    }

    /// Asserts that a drag in flight keeps the resize cursor of its own
    /// divider while the pointer crosses the grab band of another divider
    /// running the other way.
    ///
    /// Case: the user drags a column divider, and the pointer passes over
    /// a row divider inside the neighbouring pane.
    #[test]
    fn a_held_drag_keeps_its_own_resize_cursor_over_another_divider() {
        let mut app = divider_hover_app(2.0, Vec2::new(20.0, 40.0));
        app.world_mut().spawn((
            OrzmuxSeparator {
                split: SplitId(1),
                orientation: SplitOrientation::Vertical,
            },
            GrabbedSeparator::held(SplitId(1), SplitOrientation::Vertical),
            ComputedNode {
                size: Vec2::new(2.0, 160.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(400.0, 80.0),
        ));
        app.world_mut().spawn((
            OrzmuxSeparator {
                split: SplitId(2),
                orientation: SplitOrientation::Horizontal,
            },
            ComputedNode {
                size: Vec2::new(160.0, 2.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(80.0, 40.0),
        ));

        app.update();

        assert_eq!(
            window_cursor(&mut app),
            Some(CursorIcon::System(SystemCursorIcon::ColResize)),
            "the held column divider decides the cursor even over a row divider's band"
        );
        assert_eq!(
            app.world().resource::<HyperlinkHoverState>().entity,
            None,
            "a drag in flight hovers no terminal"
        );
    }
}
