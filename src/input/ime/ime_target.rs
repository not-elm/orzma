//! Resolves which IME target the window policy serves from the current
//! keyboard focus.

use super::resolve_focused_surface;
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
use bevy_orzma_webview::{Webview, focused_webview_of};

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
    webview_parents: Query<'w, 's, &'static ChildOf, With<Webview>>,
    webview_slots: Query<'w, 's, &'static Webview>,
    overlays: Query<'w, 's, &'static TerminalOverlays>,
}

impl ImePolicyParams<'_, '_> {
    pub fn decide(&self, scale_factor: f32) -> ImeTarget {
        if self.rename.as_deref().is_some_and(TabRename::is_active) {
            return ImeTarget::Rename;
        }
        // NOTE: the webview check must stay ahead of the vi-mode and no-surface
        // checks. A focused CEF webview receives IME input through bevy_cef's `Ime`
        // bridge; switching the IME off makes bevy_winit call
        // `set_ime_allowed(false)`, so the OS delivers no `Ime` events and webview
        // IME input silently breaks.
        if self.focused_webview.0.is_some() {
            return ImeTarget::Webview(self.webview_anchor(scale_factor));
        }
        let Some(surface) = resolve_focused_surface(&self.focused) else {
            return ImeTarget::Disabled;
        };
        if self.vi_modes.contains(surface) {
            return ImeTarget::Disabled;
        }
        ImeTarget::Terminal(self.cursor_cell_rect(surface, scale_factor))
    }

    /// The focused inline webview's overlay rect origin in logical pixels.
    ///
    /// `None` when no terminal holds keyboard focus, the focused webview is
    /// not a child of that terminal, the terminal has no laid-out node, or
    /// the webview's overlay slot is inactive.
    fn webview_anchor(&self, scale_factor: f32) -> Option<Vec2> {
        let terminal = resolve_focused_surface(&self.focused)?;
        let child = focused_webview_of(
            Some(&self.focused_webview),
            &self.webview_parents,
            Some(terminal),
        )?;
        let slot = self.webview_slots.get(child).ok()?.slot();
        let (node, transform, _) = self.anchors.get(terminal).ok()?;
        let rect = *self
            .overlays
            .get(terminal)
            .ok()?
            .rects
            .get(usize::from(slot))?;
        if rect.z == 0 {
            return None;
        }
        let rect_origin_phys =
            Vec2::new(rect.y as f32, rect.x as f32) * self.metrics.metrics.cell_size;
        Some(
            (node_top_left_phys(node, transform) + rect_origin_phys)
                / scale_factor.max(f32::EPSILON),
        )
    }

    fn cursor_cell_rect(&self, surface: Entity, scale_factor: f32) -> Option<Rect> {
        let (node, transform, view) = self.anchors.get(surface).ok()?;
        let (col, row) = view.cursor_viewport_cell_or_top();
        let cell_size = self.metrics.metrics.cell_size;
        let origin_phys =
            node_top_left_phys(node, transform) + Vec2::new(col.into(), row.into()) * cell_size;
        let scale = scale_factor.max(f32::EPSILON);
        Some(Rect::from_corners(
            origin_phys / scale,
            (origin_phys + cell_size) / scale,
        ))
    }
}

pub(super) enum ImeTarget {
    /// A tab rename is in progress, and the rename field positions the
    /// candidate window.
    Rename,
    /// A webview holds keyboard focus; the anchor is in logical pixels and
    /// is `None` when no anchor can be derived.
    Webview(Option<Vec2>),
    /// A terminal outside vi mode holds keyboard focus; the rectangle is its
    /// cursor cell in logical pixels and is `None` when the terminal has no
    /// laid-out node.
    Terminal(Option<Rect>),
    /// Nothing that takes IME input holds keyboard focus, or the focused
    /// terminal is in vi mode.
    Disabled,
}

/// The top-left corner of `node`, in physical pixels of window space.
///
/// Window space has its origin at the window's top-left corner and its y
/// axis pointing down. `transform` places the center of `node` in that
/// space, and `node`'s size is measured in the same physical pixels. The
/// result is not divided by the window's scale factor; a caller that needs
/// logical pixels divides it. A node whose `UiTransform` rotates or scales
/// it gets a point that is not the corner it is drawn at.
///
/// ```text
///  (0, 0)
///    ┼───────────────────────────────────────────▶ x
///    │
///    │   ●─────────────────────┐ ← the returned corner
///    │   │                     │
///    │   │          ×          │ ← transform.translation
///    │   │                     │
///    │   └─────────────────────┘
///    │   ←─── node.size().x ───→
///    ▼
///    y
/// ```
///
/// # Examples
///
/// A 1600 × 1200 node whose center sits at (1000, 700) has its top-left
/// corner at (200, 100).
fn node_top_left_phys(node: &ComputedNode, transform: &UiGlobalTransform) -> Vec2 {
    transform.translation - 0.5 * node.size()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::ime::test_support::{
        focus_inline_webview, policy_app, spawn_focused_surface,
    };
    use bevy::app::App;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::IVec4;
    use bevy_orzmux::prelude::TabId;

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

    /// Asserts that a cursor scrolled out of the viewport anchors the
    /// candidate area on the top row, in the cursor's column.
    ///
    /// Case: the user scrolls back through history and starts composing while
    /// the prompt's row is out of view.
    #[test]
    fn a_cursor_scrolled_out_of_view_anchors_the_candidate_area_on_the_top_row() {
        let mut app = policy_app();
        let surface = spawn_focused_surface(&mut app);
        app.world_mut()
            .get_mut::<TerminalView>(surface)
            .expect("the surface carries a view")
            .display_offset = 20;

        let target = decide_in(&mut app, 2.0);

        let ImeTarget::Terminal(Some(rect)) = target else {
            panic!("a focused terminal outside vi mode yields a candidate area");
        };
        assert!(
            rect.min.abs_diff_eq(Vec2::new(70.0, 50.0), 1e-3),
            "the area starts on the top row in the cursor's column, got {:?}",
            rect.min,
        );
        assert!(
            rect.size().abs_diff_eq(Vec2::new(4.0, 8.0), 1e-3),
            "the area is one cell in logical px, got {:?}",
            rect.size(),
        );
    }
}
