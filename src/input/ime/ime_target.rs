//! Resolves which IME target the window policy serves from the current
//! keyboard focus.

use crate::action::vi::mode::ViModeState;
use crate::input::focus::KeyboardFocused;
use crate::ui::tab_bar::rename::TabRename;
use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::query::With;
use bevy::ecs::system::{Query, Res, SystemParam};
use bevy::math::{Rect, Vec2};
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy_cef::prelude::FocusedWebview;
use bevy_orzma_tty_renderer::prelude::{
    TerminalCellMetricsResource, TerminalOverlays, TerminalView,
};
use bevy_orzma_webview::Webview;

#[derive(SystemParam)]
pub(super) struct ImePolicyParams<'w, 's> {
    rename: Option<Res<'w, TabRename>>,
    focused: Query<'w, 's, Entity, With<KeyboardFocused>>,
    vi_modes: Query<'w, 's, (), With<ViModeState>>,
    anchors: Query<
        'w,
        's,
        (
            &'static ComputedNode,
            &'static UiGlobalTransform,
            &'static TerminalView,
        ),
    >,
    metrics: Res<'w, TerminalCellMetricsResource>,
    focused_webview: Res<'w, FocusedWebview>,
    webview_anchors: Query<'w, 's, (&'static ComputedNode, &'static UiGlobalTransform)>,
    webview_parents: Query<'w, 's, &'static ChildOf, With<Webview>>,
    webview_slots: Query<'w, 's, &'static Webview>,
    overlays: Query<'w, 's, &'static TerminalOverlays>,
}

impl ImePolicyParams<'_, '_> {
    pub fn decide(&self, scale_factor: f32) -> ImeTarget {
        todo!("現在の入力状態などからImeTargetへの解決")
    }
}

pub(super) enum ImeTarget {
    Rename,                 // ime_enabled = true。ime_position は Bevy の入力欄に任せる
    Webview(Option<Vec2>),  // ime_enabled = true。位置が取れれば ime_position に書く
    Terminal(Option<Rect>), // ime_enabled = true。矩形が取れれば起点を ime_position、area は Some
    Disabled,               // ime_enabled = false(vi モード、フォーカスなし)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::OrzmaTerminal;
    use bevy::app::App;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::IVec4;
    use bevy::prelude::{MinimalPlugins, default};
    use bevy::state::app::StatesPlugin;
    use bevy_orzma_tty_renderer::prelude::{Baseline, CellMetrics, Thickness, Underline};
    use bevy_orzmux::prelude::TabId;
    use orzma_vt::prelude::{Cursor, GridColumn, GridLine, GridPoint, InstanceId};
    use orzma_webview_host::prelude::MountId;

    fn policy_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.init_resource::<FocusedWebview>();
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                cell_size: Vec2::new(8.0, 16.0),
                baseline: Baseline::new(12.0),
                underline: Underline {
                    position: -2.0,
                    thickness: Thickness::new(1.0),
                },
                max_overflow: 0.0,
            },
            phys_font_size: 12,
        });
        app
    }

    /// Spawns a keyboard-focused terminal whose node's top-left corner sits
    /// at (100, 100) physical px, with the cursor at column 5, row 10 of a
    /// 80×24 viewport.
    fn spawn_focused_surface(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                OrzmaTerminal,
                KeyboardFocused,
                ComputedNode {
                    size: Vec2::new(800.0, 600.0),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(500.0, 400.0),
                TerminalView {
                    cols: 80,
                    rows: 24,
                    cursor: Some(Cursor {
                        point: GridPoint {
                            line: GridLine(10),
                            column: GridColumn(5),
                        },
                        ..Cursor::default()
                    }),
                    ..default()
                },
                TerminalOverlays::default(),
            ))
            .id()
    }

    /// Gives `surface`'s overlay slot 0 the cell rect `rect`
    /// (`row, col, rows, cols`), spawns a webview on that slot as its child,
    /// and focuses the webview.
    fn focus_inline_webview(app: &mut App, surface: Entity, rect: IVec4) -> Entity {
        app.world_mut()
            .get_mut::<TerminalOverlays>(surface)
            .expect("the surface carries overlays")
            .rects[0] = rect;
        let webview = app
            .world_mut()
            .spawn((
                ChildOf(surface),
                Webview::new("webview".into(), InstanceId(1), MountId::new(1), 0, 10, 40),
            ))
            .id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        webview
    }

    fn decide_in(app: &mut App, scale_factor: f32) -> ImeTarget {
        app.world_mut()
            .run_system_once(move |params: ImePolicyParams| params.decide(scale_factor))
            .expect("ImePolicyParams resolves against the test world")
    }

    /// Asserts that a tab rename in progress yields the rename target even
    /// while a terminal outside vi mode holds keyboard focus.
    ///
    /// Case: the user double-clicks a tab to rename it while the shell pane
    /// below still holds keyboard focus.
    #[test]
    fn a_tab_rename_in_progress_yields_the_rename_target() {
        let mut app = policy_app();
        app.insert_resource(TabRename::active_for_test(TabId(1)));
        spawn_focused_surface(&mut app);

        let target = decide_in(&mut app, 1.0);

        assert!(matches!(target, ImeTarget::Rename));
    }

    /// Asserts that an inline webview anchors at its overlay rect origin,
    /// converted to logical pixels.
    ///
    /// Case: the user clicks into an inline webview the active pane mounted
    /// at row 2, column 3, on a display with a scale factor of 2.
    #[test]
    fn an_inline_webview_anchors_at_its_overlay_rect_origin() {
        let mut app = policy_app();
        let surface = spawn_focused_surface(&mut app);
        focus_inline_webview(&mut app, surface, IVec4::new(2, 3, 4, 10));

        let target = decide_in(&mut app, 2.0);

        let ImeTarget::Webview(Some(pos)) = target else {
            panic!("an inline webview with a live overlay rect yields its anchor");
        };
        assert!(
            pos.abs_diff_eq(Vec2::new(62.0, 66.0), 1e-3),
            "the anchor is (node top-left + (col × cell_w, row × cell_h)) / scale, got {pos:?}",
        );
    }

    /// Asserts that the IME is disabled when neither a terminal nor a
    /// webview holds keyboard focus.
    ///
    /// Case: the window has just opened and no pane has taken focus yet.
    #[test]
    fn no_focused_surface_and_no_webview_disables_the_ime() {
        let mut app = policy_app();

        let target = decide_in(&mut app, 1.0);

        assert!(matches!(target, ImeTarget::Disabled));
    }

    /// Asserts that two keyboard-focused terminals degrade to no focused
    /// surface, which disables the IME.
    ///
    /// Case: a focus handoff between two panes momentarily leaves both
    /// marked as keyboard-focused.
    #[test]
    fn two_keyboard_focused_surfaces_disable_the_ime() {
        let mut app = policy_app();
        spawn_focused_surface(&mut app);
        spawn_focused_surface(&mut app);

        let target = decide_in(&mut app, 1.0);

        assert!(matches!(target, ImeTarget::Disabled));
    }

    /// Asserts that a keyboard-focused terminal in vi mode disables the IME.
    ///
    /// Case: the user enters vi mode in the focused pane to select
    /// scrollback text with the keyboard.
    #[test]
    fn a_focused_surface_in_vi_mode_disables_the_ime() {
        let mut app = policy_app();
        let surface = spawn_focused_surface(&mut app);
        app.world_mut().entity_mut(surface).insert(ViModeState);

        let target = decide_in(&mut app, 1.0);

        assert!(matches!(target, ImeTarget::Disabled));
    }

    /// Asserts that a keyboard-focused terminal outside vi mode yields its
    /// cursor cell, in logical pixels, as the candidate area.
    ///
    /// Case: the user starts composing Japanese text at the shell prompt on
    /// a display with a scale factor of 2.
    #[test]
    fn a_focused_surface_anchors_the_candidate_area_at_its_cursor_cell() {
        let mut app = policy_app();
        spawn_focused_surface(&mut app);

        let target = decide_in(&mut app, 2.0);

        let ImeTarget::Terminal(Some(rect)) = target else {
            panic!("a focused terminal outside vi mode yields a candidate area");
        };
        assert!(
            rect.min.abs_diff_eq(Vec2::new(70.0, 130.0), 1e-3),
            "the area starts at the cursor cell's top-left in logical px, got {:?}",
            rect.min,
        );
        assert!(
            rect.size().abs_diff_eq(Vec2::new(4.0, 8.0), 1e-3),
            "the area is one cell in logical px, got {:?}",
            rect.size(),
        );
    }

    /// Asserts that a tab rename asked to end this frame still yields the
    /// rename target.
    ///
    /// Case: the user presses Enter to commit a new tab name, and the policy
    /// runs in the same frame before the rename finishes.
    #[test]
    fn a_tab_rename_asked_to_end_this_frame_still_yields_the_rename_target() {
        let mut app = policy_app();
        app.insert_resource(TabRename::ending_for_test(
            TabId(1),
            Entity::PLACEHOLDER,
            Entity::PLACEHOLDER,
            true,
        ));
        spawn_focused_surface(&mut app);

        let target = decide_in(&mut app, 1.0);

        assert!(matches!(target, ImeTarget::Rename));
    }

    /// Asserts that an inline webview whose overlay slot is the inactive
    /// sentinel keeps the IME enabled but yields no anchor.
    ///
    /// Case: the user focuses an inline webview in the frame its pane
    /// released the overlay slot, before the focus moves away.
    #[test]
    fn an_inline_webview_on_an_inactive_overlay_slot_leaves_the_anchor_unset() {
        let mut app = policy_app();
        let surface = spawn_focused_surface(&mut app);
        focus_inline_webview(&mut app, surface, IVec4::new(2, 3, 0, 10));

        let target = decide_in(&mut app, 2.0);

        assert!(matches!(target, ImeTarget::Webview(None)));
    }

    /// Asserts that a focused inline webview keeps the IME enabled even when
    /// its terminal is in vi mode.
    ///
    /// Case: the user enters vi mode in a pane, then clicks into the inline
    /// webview that pane mounted.
    #[test]
    fn a_focused_inline_webview_keeps_the_ime_enabled_over_a_vi_mode_surface() {
        let mut app = policy_app();
        let surface = spawn_focused_surface(&mut app);
        app.world_mut().entity_mut(surface).insert(ViModeState);
        focus_inline_webview(&mut app, surface, IVec4::new(2, 3, 4, 10));

        let target = decide_in(&mut app, 2.0);

        assert!(matches!(target, ImeTarget::Webview(Some(_))));
    }
}
