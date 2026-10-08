//! Link hover (OSC 8 and plain-text URLs) and cursor-icon control: this module
//! alone writes `HyperlinkHoverState`, and over an inline webview that owns the
//! pointer it leaves `CursorIcon` to CEF except to restore CEF's last cursor on entry.

use crate::input::bindings::OrzmaMouseConfig;
use crate::input::focus::{MouseClaimedByWebview, TerminalMouseDisabled, WebviewMouseDisabled};
use crate::input::mouse::TerminalSurfaces;
use crate::input::mouse::separator::{GrabbedSeparator, SeparatorHit, SeparatorNodes};
use crate::input::{InputPhase, current_modifiers, link_modifier_held};
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
use bevy_orzma_tty_renderer::prelude::{
    HyperlinkHoverState, TerminalCellMetricsResource, TerminalCells, TerminalView,
};
use bevy_orzmux::prelude::{OrzmuxSeparator, PaneGeometry, SplitOrientation};
use orzma_vt::prelude::{GridColumn, ViewportLine, ViewportPoint};

/// Adds hyperlink hover detection and cursor-icon control for every
/// terminal surface.
pub(super) struct HyperlinkInputPlugin;

impl Plugin for HyperlinkInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CefCursor>()
            .add_message::<CursorIconInserted>()
            .add_systems(Startup, watch_primary_window_cursor)
            .add_systems(
                Update,
                hyperlink_hover_and_cursor
                    .run_if(hover_needs_refresh())
                    .in_set(InputPhase::Hover),
            );
    }
}

/// True on a frame that can change what the pointer hovers or what the
/// cursor shows: pointer motion, a key (including the releases Bevy sends
/// for every held key when the window loses keyboard focus), a cursor
/// inserted on the window, or new cells in the pane under the pointer
/// while the link modifier is held. Every condition is evaluated on every
/// frame.
fn hover_needs_refresh() -> impl SystemCondition<()> {
    on_message::<MouseMotion>
        .or_eager(on_message::<CursorMoved>)
        .or_eager(on_message::<KeyboardInput>)
        .or_eager(on_message::<CursorIconInserted>)
        .or_eager(hovered_cells_changed)
}

/// Whether the cells of the pane under the pointer changed while the link
/// modifier is held.
fn hovered_cells_changed(
    hover: Res<HyperlinkHoverState>,
    cells: Query<Ref<TerminalCells>>,
) -> bool {
    hover.modifier_held
        && hover
            .entity
            .and_then(|entity| cells.get(entity).ok())
            .is_some_and(|cells| cells.is_changed())
}

/// Every terminal surface, with the input gates that decide who owns the
/// pointer over it.
type HoverSurfaces<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ComputedNode,
        &'static ComputedStackIndex,
        &'static UiGlobalTransform,
        Has<TerminalMouseDisabled>,
        Has<MouseClaimedByWebview>,
        Has<WebviewMouseDisabled>,
    ),
    With<OrzmaTerminal>,
>;

/// Over a surface whose inline webview owns the pointer
/// (`MouseClaimedByWebview` without `WebviewMouseDisabled`), leaves the cursor
/// to CEF and writes CEF's last cursor once on the frame the pointer enters.
/// Over a claimed surface whose webview input is disabled, or an unclaimed
/// surface with `TerminalMouseDisabled`, shows the arrow. Neither advertises a
/// link. A divider the pointer holds or hovers claims the cursor before any
/// surface is read, leaving the hover state empty; a held divider keeps the
/// cursor even while the pointer reports no position, which is what a drag past
/// the window's edge does. While the pointer reports no position, the hover
/// state is cleared, `modifier_held` included. The hover state is written only
/// when it changes.
fn hyperlink_hover_and_cursor(
    mut hover: ResMut<HyperlinkHoverState>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    mut was_over_webview: Local<bool>,
    windows: Query<&Window, With<PrimaryWindow>>,
    targets: HoverTargetParams,
    cef_cursor: Res<CefCursor>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Some(cursor_phys) = windows
        .single()
        .ok()
        .and_then(|window| Some(window.cursor_position()? * window.scale_factor()))
    else {
        hover.set_if_neq(HyperlinkHoverState::default());
        *was_over_webview = false;
        apply_cursor(&mut cursor_icons, cursor_decision(targets.unlocated()));
        return;
    };

    let mut next = HyperlinkHoverState {
        modifier_held: link_modifier_held(&current_modifiers(&keys)),
        ..HyperlinkHoverState::default()
    };
    let target = targets.target(&mut next, cursor_phys);
    hover.set_if_neq(next);
    let over_webview = matches!(target, HoverTarget::Webview);
    let entering_webview = over_webview && !*was_over_webview;
    *was_over_webview = over_webview;
    let decision = if entering_webview {
        cef_cursor.0
    } else {
        cursor_decision(target)
    };
    apply_cursor(&mut cursor_icons, decision);
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
/// `Webview` is a page that owns the pointer, whose cursor is CEF's.
/// `Default` covers everything else that is not a terminal grid: chrome,
/// gaps, an unobservable window, and surfaces whose mouse input is
/// suppressed.
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
    /// The surfaces a new press can reach, the same set a divider grab needs.
    pressable: TerminalSurfaces<'w, 's>,
    terminals: Query<'w, 's, (&'static TerminalView, &'static TerminalCells)>,
    separators: SeparatorNodes<'w, 's>,
    grabbed: Query<'w, 's, &'static OrzmuxSeparator, With<GrabbedSeparator>>,
    metrics: Res<'w, TerminalCellMetricsResource>,
    geometry: Option<Res<'w, PaneGeometry>>,
    mouse: Res<'w, OrzmaMouseConfig>,
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
    /// geometry is unknown or no surface accepts a new press.
    fn hovered(&self, cursor_phys: Vec2) -> Option<SplitOrientation> {
        let geometry = self.geometry.as_deref()?;
        if self.pressable.is_empty() {
            return None;
        }
        SeparatorHit::at(
            cursor_phys,
            geometry,
            self.mouse.divider_grab_tolerance_px,
            self.separators.iter(),
        )
        .map(|hit| hit.orientation)
    }

    /// The region for the topmost terminal surface under `cursor_phys`, in
    /// window physical px. Over a terminal grid whose mouse input is live,
    /// records in `hover` that surface, the hyperlink id of the cell under the
    /// pointer, and, while the link modifier is held over a cell without one,
    /// the detected URL that cell shows; elsewhere leaves them untouched.
    fn over_surface(&self, hover: &mut HyperlinkHoverState, cursor_phys: Vec2) -> HoverTarget {
        let candidates = self
            .surfaces
            .iter()
            .map(|(entity, node, stack, transform, ..)| (entity, node, stack, transform));
        let Some(entity) = topmost_surface_at(cursor_phys, candidates) else {
            return HoverTarget::Default;
        };
        let Ok((_, node, _, transform, mouse_disabled, claimed, webview_disabled)) =
            self.surfaces.get(entity)
        else {
            return HoverTarget::Default;
        };
        if claimed {
            return if webview_disabled {
                HoverTarget::Default
            } else {
                HoverTarget::Webview
            };
        }
        if mouse_disabled {
            return HoverTarget::Default;
        }
        let Ok((view, cells)) = self.terminals.get(entity) else {
            return HoverTarget::Default;
        };
        let (cell_w, cell_h) = cell_pitch_phys(&self.metrics.metrics);
        let cell = phys_to_pane_local(node, transform, cursor_phys)
            .map(|local| cell_at_local(local, cell_w, cell_h, view.cols, view.rows))
            .map(|(col, row, _side)| (row.saturating_sub(1) as u16, col.saturating_sub(1) as u16));
        let id = cell
            .and_then(|(row, col)| cells.hyperlink_at(row, col))
            .map(|(id, _uri)| id);
        let detected = cell.filter(|_| hover.modifier_held).and_then(|(row, col)| {
            cells.detected_url_at(ViewportPoint {
                line: ViewportLine(row),
                column: GridColumn(col),
            })
        });
        let has_link = id.is_some() || detected.is_some();
        hover.entity = Some(entity);
        hover.hyperlink_id = id;
        hover.detected = detected;
        HoverTarget::Terminal {
            has_link,
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

/// The system cursor last inserted on the primary window: CEF's latest
/// choice, or the startup arrow before CEF asked for any.
// TODO: drop this and the restore on entering a page once the webview
// router sends CEF a mouse-leave when the pointer exits a page; Blink then
// reports its cursor again on re-entry.
#[derive(Resource, Default)]
struct CefCursor(Option<SystemCursorIcon>);

/// An insert of `CursorIcon` on the primary window, which may have replaced
/// the cursor the hover system chose.
#[derive(Message)]
struct CursorIconInserted;

/// Records the system cursor inserted on the primary window as CEF's latest
/// choice, and reports every insert as a `CursorIconInserted`.
fn record_cef_cursor(
    ev: On<Insert, CursorIcon>,
    mut cef_cursor: ResMut<CefCursor>,
    mut inserted: MessageWriter<CursorIconInserted>,
    windows: Query<&CursorIcon, With<PrimaryWindow>>,
) {
    // NOTE: only `bevy_cef` (and the startup arrow) insert `CursorIcon`; the
    // hover system writes it through `DerefMut`, which fires no `Insert`. A
    // hover write that inserted instead would be recorded as CEF's choice.
    if let Ok(CursorIcon::System(icon)) = windows.get(ev.event_target())
        && cef_cursor.0 != Some(*icon)
    {
        cef_cursor.0 = Some(*icon);
    }
    inserted.write(CursorIconInserted);
}

/// Attaches `record_cef_cursor` to the primary window, then inserts the
/// arrow on a window that has no `CursorIcon` yet, so the hover system can
/// mutate the component without inserting it. The observer records that
/// arrow as `CefCursor`'s first value.
fn watch_primary_window_cursor(
    mut commands: Commands,
    windows: Query<(Entity, Has<CursorIcon>), With<PrimaryWindow>>,
) {
    for (window, has_cursor) in windows.iter() {
        let mut window = commands.entity(window);
        // NOTE: the observer must be attached before the arrow is inserted, or
        // `CefCursor` stays `None` and the first entry into a page restores
        // nothing.
        window.observe(record_cef_cursor);
        if !has_cursor {
            window.insert(CursorIcon::System(SystemCursorIcon::Default));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::hold_link_modifier;
    use bevy_orzmux::prelude::SplitId;
    use orzma_vt::prelude::{DetectedUrl, HyperlinkId, HyperlinkUri};

    fn id(value: u32) -> HyperlinkId {
        HyperlinkId::new(value).expect("nonzero")
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

    use bevy_orzma_tty_renderer::prelude::{CellMetrics, LineStroke};

    fn hover_test_metrics() -> TerminalCellMetricsResource {
        TerminalCellMetricsResource {
            metrics: CellMetrics {
                cell_size: Vec2::new(8.0, 16.0),
                baseline: 12.0,
                underline: LineStroke {
                    position: -2.0,
                    thickness: 1.0,
                },
                max_overflow: 0.0,
            },
            phys_font_size: 16,
        }
    }

    /// Asserts that with no surface under the pointer the hover state is
    /// cleared and the cursor shows the arrow.
    ///
    /// Case: the pane whose link the pointer rested on closes, and the pointer
    /// then moves over the empty window.
    #[test]
    fn hover_with_no_panes_leaves_entity_none_and_cursor_default() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseMotion>();
        app.init_resource::<HyperlinkHoverState>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(hover_test_metrics());
        app.init_resource::<CefCursor>();
        app.init_resource::<OrzmaMouseConfig>();
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
        use orzma_vt::prelude::{Cell, HyperlinkUri};
        use std::collections::HashMap;
        let mut rows = vec![vec![Cell::default(); 10]; 5];
        rows[0][0] = Cell {
            c: 'x',
            hyperlink_id: Some(id(7)),
            ..Cell::default()
        };
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
        app.init_resource::<CefCursor>();
        app.init_resource::<OrzmaMouseConfig>();
        app.add_systems(Update, hyperlink_hover_and_cursor);

        hold_link_modifier(&mut app);

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

    /// Asserts that a `TerminalMouseDisabled` surface is never hovered: the hover
    /// state stays empty and the cursor shows the default arrow even over a
    /// linked cell.
    ///
    /// Case: the pointer crosses a hyperlink on a terminal whose mouse input
    /// is suppressed while an IME composition is in progress.
    #[test]
    fn hover_skips_terminal_mouse_disabled_surface() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseMotion>();
        app.init_resource::<HyperlinkHoverState>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(hover_test_metrics());
        app.init_resource::<CefCursor>();
        app.init_resource::<OrzmaMouseConfig>();
        app.add_systems(Update, hyperlink_hover_and_cursor);

        hold_link_modifier(&mut app);

        let mut window = Window::default();
        window.set_cursor_position(Some(Vec2::new(4.0, 8.0)));
        let window_entity = app
            .world_mut()
            .spawn((
                window,
                PrimaryWindow,
                CursorIcon::System(SystemCursorIcon::Grab),
            ))
            .id();

        let (view, cells) = linked_grid();
        app.world_mut().spawn((
            OrzmaTerminal,
            TerminalMouseDisabled,
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
            "a TerminalMouseDisabled surface must not be hovered — the click is suppressed, so no link affordance"
        );
        assert_eq!(hover.hyperlink_id, None);
        let icon = app.world().entity(window_entity).get::<CursorIcon>();
        assert_eq!(
            icon,
            Some(&CursorIcon::System(SystemCursorIcon::Default)),
            "with input suppressed the cursor shows the arrow, not a link pointer"
        );
    }

    /// A hover world with the pointer at `cursor` (logical px), an 8x16 px
    /// cell, a primary window carrying the CEF-cursor recorder whose cursor
    /// starts as `start` (recorded as CEF's), and two 80x80 terminal surfaces
    /// side by side: a plain one at x 0..80 and one at x 80..160 carrying
    /// `gates`. Returns the app and the window entity.
    fn gated_hover_app(cursor: Vec2, start: SystemCursorIcon, gates: impl Bundle) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .init_resource::<HyperlinkHoverState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<CefCursor>()
            .init_resource::<OrzmaMouseConfig>()
            .add_message::<CursorIconInserted>()
            .insert_resource(hover_test_metrics())
            .add_systems(Update, hyperlink_hover_and_cursor);
        let mut window = Window::default();
        window.set_cursor_position(Some(cursor));
        let window_entity = app
            .world_mut()
            .spawn((window, PrimaryWindow))
            .observe(record_cef_cursor)
            .insert(CursorIcon::System(start))
            .id();
        let (view, cells) = linked_grid();
        app.world_mut().spawn((
            OrzmaTerminal,
            ComputedNode {
                size: Vec2::new(80.0, 80.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(40.0, 40.0),
            view,
            cells,
        ));
        let (view, cells) = linked_grid();
        app.world_mut().spawn((
            OrzmaTerminal,
            gates,
            ComputedNode {
                size: Vec2::new(80.0, 80.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(120.0, 40.0),
            view,
            cells,
        ));
        (app, window_entity)
    }

    fn move_cursor(app: &mut App, window: Entity, cursor: Option<Vec2>) {
        app.world_mut()
            .get_mut::<Window>(window)
            .expect("the hover world has a primary window")
            .set_cursor_position(cursor);
    }

    fn cursor_of(app: &App, window: Entity) -> Option<CursorIcon> {
        app.world().get::<CursorIcon>(window).cloned()
    }

    /// Asserts that the startup system gives the primary window the arrow and
    /// records as CEF's choice only the cursors inserted on that window.
    ///
    /// Case: orzma starts, and bevy_cef later inserts a page's cursor on every
    /// entity in the world, the primary window among them.
    #[test]
    fn only_cursors_inserted_on_the_primary_window_are_recorded() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<CefCursor>()
            .add_message::<CursorIconInserted>()
            .add_systems(Startup, watch_primary_window_cursor);
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        let other = app.world_mut().spawn_empty().id();
        app.update();
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Default))
        );
        assert_eq!(
            app.world().resource::<CefCursor>().0,
            Some(SystemCursorIcon::Default)
        );

        app.world_mut()
            .entity_mut(other)
            .insert(CursorIcon::System(SystemCursorIcon::Wait));
        assert_eq!(
            app.world().resource::<CefCursor>().0,
            Some(SystemCursorIcon::Default),
            "an insert on another entity is not CEF's choice for the window"
        );
        app.world_mut()
            .entity_mut(window)
            .insert(CursorIcon::System(SystemCursorIcon::Pointer));
        assert_eq!(
            app.world().resource::<CefCursor>().0,
            Some(SystemCursorIcon::Pointer)
        );
    }

    /// Asserts that over a surface whose inline webview owns the pointer,
    /// hover stays empty and the cursor CEF set is left in place.
    ///
    /// Case: the user moves the pointer across a link on a page mounted in
    /// a pane, and CEF has just switched the cursor to the pointing hand.
    #[test]
    fn hover_leaves_the_cursor_to_cef_over_a_claimed_surface() {
        let (mut app, window) = gated_hover_app(
            Vec2::new(84.0, 8.0),
            SystemCursorIcon::Pointer,
            MouseClaimedByWebview,
        );
        app.update();
        app.update();
        let hover = app.world().resource::<HyperlinkHoverState>();
        assert_eq!(hover.entity, None);
        assert_eq!(hover.hyperlink_id, None);
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Pointer))
        );
    }

    /// Asserts that a page owning an IME composition keeps the cursor CEF set
    /// and advertises no link, even though the terminal's own mouse input is
    /// suppressed.
    ///
    /// Case: the user is converting Japanese text in a text field on a page
    /// mounted in a pane and moves the pointer over a link on that page.
    #[test]
    fn a_page_owning_a_composition_keeps_the_cef_cursor() {
        let (mut app, window) = gated_hover_app(
            Vec2::new(84.0, 8.0),
            SystemCursorIcon::Pointer,
            (MouseClaimedByWebview, TerminalMouseDisabled),
        );
        app.update();
        let hover = app.world().resource::<HyperlinkHoverState>();
        assert_eq!(hover.entity, None);
        assert_eq!(hover.hyperlink_id, None);
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Pointer))
        );
    }

    /// Asserts that a claimed surface whose webview input is also disabled
    /// shows the arrow and advertises no link.
    ///
    /// Case: the user enters vi mode in a pane that has a page mounted and
    /// moves the pointer over that page.
    #[test]
    fn a_claimed_surface_with_webview_input_disabled_shows_the_arrow() {
        let (mut app, window) = gated_hover_app(
            Vec2::new(84.0, 8.0),
            SystemCursorIcon::Pointer,
            (
                MouseClaimedByWebview,
                WebviewMouseDisabled,
                TerminalMouseDisabled,
            ),
        );
        app.update();
        assert_eq!(app.world().resource::<HyperlinkHoverState>().entity, None);
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Default))
        );
    }

    /// Asserts that the cursor CEF last inserted is written back once on the
    /// frame the pointer enters the page, and not again while it stays.
    ///
    /// Case: the user moves from the shell prompt onto a page whose link had
    /// shown the pointing hand before, then lingers on the page.
    #[test]
    fn entering_a_page_restores_the_last_cef_cursor_once() {
        let (mut app, window) = gated_hover_app(
            Vec2::new(4.0, 8.0),
            SystemCursorIcon::Default,
            MouseClaimedByWebview,
        );
        app.world_mut()
            .entity_mut(window)
            .insert(CursorIcon::System(SystemCursorIcon::Pointer));
        app.update();
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Text)),
            "over the plain terminal the I-beam is written"
        );

        move_cursor(&mut app, window, Some(Vec2::new(84.0, 8.0)));
        app.update();
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Pointer)),
            "entering the page restores CEF's last cursor"
        );

        *app.world_mut()
            .get_mut::<CursorIcon>(window)
            .expect("the window has a cursor") = CursorIcon::System(SystemCursorIcon::Grab);
        app.update();
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Grab)),
            "staying on the page writes nothing"
        );
    }

    /// Asserts that leaving the window re-arms the restore, so coming back
    /// onto the page writes CEF's last cursor again.
    ///
    /// Case: the user drags the pointer off the window while over a page and
    /// brings it back onto the same page.
    #[test]
    fn leaving_the_window_rearms_the_restore() {
        let (mut app, window) = gated_hover_app(
            Vec2::new(84.0, 8.0),
            SystemCursorIcon::Default,
            MouseClaimedByWebview,
        );
        app.world_mut()
            .entity_mut(window)
            .insert(CursorIcon::System(SystemCursorIcon::Pointer));
        app.update();

        move_cursor(&mut app, window, None);
        app.update();
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Default)),
            "outside the window the arrow is written"
        );
        move_cursor(&mut app, window, Some(Vec2::new(84.0, 8.0)));
        app.update();
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Pointer)),
            "coming back onto the page restores CEF's last cursor"
        );
    }

    /// Asserts that moving from a page back onto terminal text writes the
    /// I-beam.
    ///
    /// Case: the user moves the pointer off a mounted page onto the shell
    /// output beside it.
    #[test]
    fn leaving_a_page_for_terminal_text_writes_the_i_beam() {
        let (mut app, window) = gated_hover_app(
            Vec2::new(84.0, 8.0),
            SystemCursorIcon::Pointer,
            MouseClaimedByWebview,
        );
        app.update();
        move_cursor(&mut app, window, Some(Vec2::new(4.0, 8.0)));
        app.update();
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Text))
        );
    }

    /// Asserts that a cursor inserted on the window while the pointer rests
    /// on terminal text is replaced by the I-beam on the next update, with no
    /// pointer or key input.
    ///
    /// Case: the user moves the pointer off a page onto the shell text beside
    /// it and stops, and CEF's report of the link the pointer just crossed
    /// arrives a frame later.
    #[test]
    fn a_late_cef_cursor_over_terminal_text_is_replaced() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<CursorMoved>()
            .add_message::<KeyboardInput>()
            .init_resource::<HyperlinkHoverState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<OrzmaMouseConfig>()
            .insert_resource(hover_test_metrics())
            .add_plugins(HyperlinkInputPlugin);
        let mut window = Window::default();
        window.set_cursor_position(Some(Vec2::new(4.0, 8.0)));
        let window = app.world_mut().spawn((window, PrimaryWindow)).id();
        let (view, cells) = linked_grid();
        app.world_mut().spawn((
            OrzmaTerminal,
            ComputedNode {
                size: Vec2::new(80.0, 80.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(40.0, 40.0),
            view,
            cells,
        ));
        app.update();

        app.world_mut()
            .entity_mut(window)
            .insert(CursorIcon::System(SystemCursorIcon::Pointer));
        app.update();

        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Text))
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
            .init_resource::<CefCursor>()
            .init_resource::<OrzmaMouseConfig>()
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

    /// Asserts that a divider's grab band shows the arrow rather than a
    /// resize cursor while no surface accepts a new press.
    ///
    /// Case: the user is converting Japanese text in the shell and moves the
    /// pointer onto the groove between two side-by-side panes.
    #[test]
    fn a_divider_shows_no_resize_cursor_while_no_surface_takes_a_press() {
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
        let terminal = app
            .world_mut()
            .query_filtered::<Entity, With<OrzmaTerminal>>()
            .single(app.world())
            .expect("the divider world has one terminal surface");
        app.world_mut()
            .entity_mut(terminal)
            .insert(TerminalMouseDisabled);

        app.update();

        assert_eq!(
            window_cursor(&mut app),
            Some(CursorIcon::System(SystemCursorIcon::Default))
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

    /// A 10x5 grid whose top row shows the plain-text URL `http://a.b`
    /// and carries no OSC 8 link.
    fn url_grid() -> (TerminalView, TerminalCells) {
        use orzma_vt::prelude::Cell;
        let mut rows = vec![vec![Cell::default(); 10]; 5];
        for (cell, c) in rows[0].iter_mut().zip("http://a.b".chars()) {
            cell.c = c;
        }
        (
            TerminalView {
                cols: 10,
                rows: 5,
                ..default()
            },
            TerminalCells {
                cells: rows,
                ..default()
            },
        )
    }

    /// A hover world with the pointer on the first cell of `url_grid`,
    /// with the link modifier held when `hold` is set; returns the app,
    /// the window and the pane.
    fn url_hover_app(hold: bool) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .init_resource::<HyperlinkHoverState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(hover_test_metrics())
            .init_resource::<CefCursor>()
            .init_resource::<OrzmaMouseConfig>()
            .add_systems(Update, hyperlink_hover_and_cursor);
        if hold {
            hold_link_modifier(&mut app);
        }
        let mut window = Window::default();
        window.set_cursor_position(Some(Vec2::new(4.0, 8.0)));
        let window = app
            .world_mut()
            .spawn((
                window,
                PrimaryWindow,
                CursorIcon::System(SystemCursorIcon::Default),
            ))
            .id();
        let (view, cells) = url_grid();
        let pane = app
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
        (app, window, pane)
    }

    /// Asserts that hovering a plain-text URL with the activation modifier
    /// held records its span and no OSC 8 id, and shows the pointer.
    ///
    /// Case: the user holds Cmd (Ctrl off macOS) over the address a
    /// development server printed.
    #[test]
    fn hovering_a_plain_url_with_the_modifier_records_its_span() {
        let (mut app, window, pane) = url_hover_app(true);
        app.update();
        let hover = app.world().resource::<HyperlinkHoverState>();
        assert_eq!(hover.entity, Some(pane));
        assert_eq!(hover.hyperlink_id, None);
        assert_eq!(
            hover.detected,
            Some(DetectedUrl {
                uri: "http://a.b".to_string(),
                first: ViewportPoint {
                    line: ViewportLine(0),
                    column: GridColumn(0),
                },
                last: ViewportPoint {
                    line: ViewportLine(0),
                    column: GridColumn(9),
                },
            })
        );
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Pointer))
        );
    }

    /// Asserts that without the activation modifier a plain-text URL is not
    /// detected and the pointer stays an I-beam.
    ///
    /// Case: the user moves the mouse across a URL in build output without
    /// holding Cmd or Ctrl.
    #[test]
    fn a_plain_url_is_not_detected_without_the_modifier() {
        let (mut app, window, _) = url_hover_app(false);
        app.update();
        assert_eq!(app.world().resource::<HyperlinkHoverState>().detected, None);
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Text))
        );
    }

    /// Asserts that an OSC 8 link on the hovered cell wins over the URL its
    /// text shows.
    ///
    /// Case: a program prints a URL as the visible text of an OSC 8 link
    /// that points elsewhere, and the user holds Cmd over it.
    #[test]
    fn an_osc8_link_wins_over_the_url_its_text_shows() {
        let (mut app, _, pane) = url_hover_app(true);
        {
            let mut cells = app
                .world_mut()
                .get_mut::<TerminalCells>(pane)
                .expect("the pane's cells");
            cells.cells[0][0].hyperlink_id = Some(id(7));
            cells
                .hyperlinks
                .insert(id(7), HyperlinkUri::new("https://osc8.example"));
        }
        app.update();
        let hover = app.world().resource::<HyperlinkHoverState>();
        assert_eq!(hover.hyperlink_id, Some(id(7)));
        assert_eq!(hover.detected, None);
    }

    /// Asserts that once the keys are released the next hover pass clears
    /// the detected span and turns the pointer back into an I-beam.
    ///
    /// Case: the user holds Cmd over a URL and presses Cmd+Tab, so the
    /// window loses keyboard focus, which releases every key.
    #[test]
    fn released_keys_clear_the_detected_span_and_the_pointer() {
        let (mut app, window, _) = url_hover_app(true);
        app.update();
        assert!(
            app.world()
                .resource::<HyperlinkHoverState>()
                .detected
                .is_some()
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();
        app.update();
        assert_eq!(app.world().resource::<HyperlinkHoverState>().detected, None);
        assert_eq!(
            cursor_of(&app, window),
            Some(CursorIcon::System(SystemCursorIcon::Text))
        );
    }

    #[derive(Resource, Default)]
    struct Refreshes(usize);

    /// An app counting the runs `hover_needs_refresh` allows, with two panes
    /// and the settling run already forgotten; returns the app, the pane
    /// the tests hover, and another pane.
    fn refresh_app() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<CursorMoved>()
            .add_message::<KeyboardInput>()
            .add_message::<CursorIconInserted>()
            .init_resource::<HyperlinkHoverState>()
            .init_resource::<Refreshes>()
            .add_systems(
                Update,
                (|mut runs: ResMut<Refreshes>| runs.0 += 1).run_if(hover_needs_refresh()),
            );
        let hovered = app.world_mut().spawn(TerminalCells::default()).id();
        let other = app.world_mut().spawn(TerminalCells::default()).id();
        app.update();
        app.world_mut().resource_mut::<Refreshes>().0 = 0;
        (app, hovered, other)
    }

    fn refreshes(app: &App) -> usize {
        app.world().resource::<Refreshes>().0
    }

    fn repaint(app: &mut App, pane: Entity) {
        app.world_mut()
            .get_mut::<TerminalCells>(pane)
            .expect("a pane")
            .cells
            .push(Vec::new());
    }

    /// Asserts that new cells in the hovered pane ask for a hover pass only
    /// while the link modifier is held, and new cells elsewhere never do.
    ///
    /// Case: the user holds Cmd over a pane where a build is printing while
    /// a second pane prints a log, and then lets go of Cmd.
    #[test]
    fn only_the_hovered_panes_new_cells_refresh_the_hover_while_the_modifier_is_held() {
        let (mut app, hovered, other) = refresh_app();
        {
            let mut hover = app.world_mut().resource_mut::<HyperlinkHoverState>();
            hover.entity = Some(hovered);
            hover.modifier_held = true;
        }
        repaint(&mut app, hovered);
        app.update();
        assert_eq!(refreshes(&app), 1, "the hovered pane repainted");
        app.update();
        assert_eq!(refreshes(&app), 1, "nothing changed");
        repaint(&mut app, other);
        app.update();
        assert_eq!(refreshes(&app), 1, "another pane repainted");
        app.world_mut()
            .resource_mut::<HyperlinkHoverState>()
            .modifier_held = false;
        repaint(&mut app, hovered);
        app.update();
        assert_eq!(refreshes(&app), 1, "the modifier is up");
    }
}
