//! Dragging a tab along the strip to reorder tabs.

use crate::ui::tab_bar::rename::TabRename;
use crate::ui::tab_bar::{
    TAB_GAP_PX, TAB_STRIP_LEFT_PADDING_PX, TabBarSystems, TabButton, TabStrip,
};
use bevy::prelude::*;
use bevy_orzmux::prelude::{
    CurrentTabs, OrzmuxSystems, PendingTabMove, RequestTabAction, TabAction, TabId,
};

/// The drag in progress, and the drop the tab bar still shows while its
/// move is unanswered.
#[derive(Resource, Default, Debug)]
pub(crate) struct TabDrag {
    active: Option<ActiveDrag>,
    dropped: Option<(TabId, usize)>,
}

impl TabDrag {
    /// The dragged tab and the slot the tabs show it in: the live drag
    /// once it passed the threshold, else the last drop still waiting for
    /// its answer.
    pub fn preview(&self) -> Option<(TabId, usize)> {
        self.active
            .filter(|drag| drag.moved)
            .map(|drag| (drag.tab, drag.target))
            .or(self.dropped)
    }

    /// Whether `tab`'s tab is being dragged past the threshold; it
    /// stays so until [`finish`](Self::finish).
    pub fn is_dragging(&self, tab: TabId) -> bool {
        self.active
            .is_some_and(|drag| drag.moved && drag.tab == tab)
    }

    /// Whether a press on `tab`'s tab is being followed, past the
    /// threshold or not.
    pub fn tracks(&self, tab: TabId) -> bool {
        self.active.is_some_and(|drag| drag.tab == tab)
    }

    /// Starts following a press on `tab`'s tab at display position
    /// `origin`, with the strip scrolled by `scroll_x` logical px.
    pub fn start(&mut self, tab: TabId, origin: usize, scroll_x: f32) {
        self.active = Some(ActiveDrag {
            tab,
            origin,
            target: origin,
            moved: false,
            scroll_at_start: scroll_x,
        });
    }

    /// Whether [`update`](Self::update) with the same `dx` and `target`
    /// would change the drag: it crosses the threshold or moves the target
    /// slot. Always `false` without a press.
    pub fn would_change(&self, dx: f32, target: usize) -> bool {
        self.active.is_some_and(|drag| {
            let moved = drag.moved || passes_threshold(dx);
            moved != drag.moved || target != drag.target
        })
    }

    /// Records the pointer's horizontal travel `dx` since the press, in
    /// logical px, and the slot `target` under it. Once `dx` reaches the
    /// threshold, the press stays a drag until it finishes.
    pub fn update(&mut self, dx: f32, target: usize) {
        if let Some(drag) = self.active.as_mut() {
            drag.moved |= passes_threshold(dx);
            drag.target = target;
        }
    }

    /// The horizontal offset, in logical px, that keeps the pressed tab
    /// under the pointer: the travel `dx` plus the scrolling since the
    /// press, minus the slots of `slot_width`, a tab and the gap after it,
    /// that the preview already moved the tab by. Returns `0.0` without a
    /// press.
    pub fn visual_offset(&self, dx: f32, slot_width: f32, scroll_x: f32) -> f32 {
        let Some(drag) = self.active else {
            return 0.0;
        };
        let shown = if drag.moved { drag.target } else { drag.origin };
        let slots = shown as f32 - drag.origin as f32;
        dx + (scroll_x - drag.scroll_at_start) - slots * slot_width
    }

    /// Ends the press. Returns the move to send when the tab travelled past
    /// the threshold to another slot; that drop stays shown until
    /// [`forget_drop`](Self::forget_drop).
    pub fn finish(&mut self) -> Option<(TabId, usize)> {
        let drag = self.active.take()?;
        let moved_to =
            (drag.moved && drag.target != drag.origin).then_some((drag.tab, drag.target));
        if moved_to.is_some() {
            self.dropped = moved_to;
        }
        moved_to
    }

    /// Whether a drop is still shown.
    pub fn has_drop(&self) -> bool {
        self.dropped.is_some()
    }

    /// Stops showing the last drop.
    pub fn forget_drop(&mut self) {
        self.dropped = None;
    }
}

/// The zero-based slot under `pointer_x` for `count` equal slots of
/// `slot_width`, each a tab and the gap after it, the first starting at
/// `first_left` in a strip scrolled by `scroll_x`; all values in logical
/// px. Returns `0` without tabs or without a width.
pub(crate) fn drop_index(
    pointer_x: f32,
    first_left: f32,
    scroll_x: f32,
    slot_width: f32,
    count: usize,
) -> usize {
    if count == 0 || slot_width <= 0.0 {
        return 0;
    }
    let slot = ((pointer_x - first_left + scroll_x) / slot_width).floor();
    slot.clamp(0.0, (count - 1) as f32) as usize
}

/// Lets a tab be dragged along the strip to reorder the tabs, and
/// stops showing a drop once its move is answered.
pub(crate) struct TabDragPlugin;

impl Plugin for TabDragPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TabDrag>()
            .add_observer(on_drag_start)
            .add_observer(on_drag)
            .add_observer(on_drag_end)
            .add_systems(
                Update,
                forget_answered_drop
                    .after(OrzmuxSystems::Drain)
                    .before(TabBarSystems::Reconcile)
                    .run_if(resource_exists_and_changed::<PendingTabMove>),
            );
    }
}

/// A press on a tab, followed from its first movement until its release.
#[derive(Debug, Clone, Copy)]
struct ActiveDrag {
    /// The tab whose tab was pressed.
    tab: TabId,
    /// The tab's display position at the press.
    origin: usize,
    /// The slot under the pointer.
    target: usize,
    /// Whether the pointer travelled past the threshold.
    moved: bool,
    /// The strip's scroll at the press, in logical px.
    scroll_at_start: f32,
}

/// Whether a horizontal travel of `dx` logical px makes a press a drag.
fn passes_threshold(dx: f32) -> bool {
    /// How far a press must travel along the strip before it is a drag, in
    /// logical px.
    const DRAG_THRESHOLD_PX: f32 = 4.0;

    dx.abs() >= DRAG_THRESHOLD_PX
}

/// Starts following a primary press on a tab once the pointer moves,
/// unless the tab is being renamed.
fn on_drag_start(
    ev: On<Pointer<DragStart>>,
    mut drag: ResMut<TabDrag>,
    buttons: Query<&TabButton>,
    strips: Query<&ScrollPosition, With<TabStrip>>,
    tabs: Res<CurrentTabs>,
    rename: Res<TabRename>,
) {
    if ev.button != PointerButton::Primary {
        return;
    }
    let Ok(tab) = buttons.get(ev.entity) else {
        return;
    };
    if rename.tab() == Some(tab.id) {
        return;
    }
    let Some(origin) = tabs.position_of(tab.id) else {
        return;
    };
    let scroll_x = strips.single().map_or(0.0, |scroll| scroll.x);
    drag.start(tab.id, origin, scroll_x);
}

/// Follows the pointer: updates the target slot, keeps the tab under the
/// pointer, and, once the press is a drag, draws the tab above its
/// neighbours and scrolls the strip near its edges.
fn on_drag(
    ev: On<Pointer<Drag>>,
    mut drag: ResMut<TabDrag>,
    mut placements: Query<(&mut UiTransform, &mut ZIndex), With<TabButton>>,
    mut strips: Query<(&ComputedNode, &UiGlobalTransform, &mut ScrollPosition), With<TabStrip>>,
    buttons: Query<(&TabButton, &ComputedNode)>,
    tabs: Res<CurrentTabs>,
) {
    /// The stacking of a tab dragged past the threshold among the strip's
    /// children, above the other tabs and the new-tab button.
    const DRAGGED_TAB_Z: ZIndex = ZIndex(1);

    if ev.button != PointerButton::Primary {
        return;
    }
    let Ok((tab, tab_node)) = buttons.get(ev.entity) else {
        return;
    };
    if !drag.tracks(tab.id) {
        return;
    }
    let Ok((strip_node, strip_transform, mut scroll)) = strips.single_mut() else {
        return;
    };
    let slot_width = tab_node.size().x * tab_node.inverse_scale_factor + TAB_GAP_PX;
    let strip_width = strip_node.size().x * strip_node.inverse_scale_factor;
    let strip_left =
        strip_transform.translation.x * strip_node.inverse_scale_factor - strip_width / 2.0;
    let first_left = strip_left + TAB_STRIP_LEFT_PADDING_PX;
    let pointer_x = ev.pointer_location.position.x;
    let dx = ev.distance.x;
    let target = drop_index(
        pointer_x,
        first_left,
        scroll.x,
        slot_width,
        tabs.entries.len(),
    );
    if drag.would_change(dx, target) {
        drag.update(dx, target);
    }
    if drag.is_dragging(tab.id) {
        let max_scroll = ((strip_node.content_size().x - strip_node.size().x)
            * strip_node.inverse_scale_factor)
            .max(0.0);
        let next = edge_scroll(pointer_x, strip_left, strip_width, scroll.x, max_scroll);
        if scroll.x != next {
            scroll.x = next;
        }
    }
    if let Ok((mut transform, mut z_index)) = placements.get_mut(ev.entity) {
        let offset = Val::Px(drag.visual_offset(dx, slot_width, scroll.x));
        if transform.translation.x != offset {
            transform.translation.x = offset;
        }
        let stacking = if drag.is_dragging(tab.id) {
            DRAGGED_TAB_Z
        } else {
            ZIndex::default()
        };
        z_index.set_if_neq(stacking);
    }
}

/// Ends a primary press on a tab: puts the tab back in the flow and in the
/// strip's stacking, and sends the move when the press was a drag to
/// another slot.
fn on_drag_end(
    ev: On<Pointer<DragEnd>>,
    mut commands: Commands,
    mut drag: ResMut<TabDrag>,
    mut placements: Query<(&mut UiTransform, &mut ZIndex), With<TabButton>>,
    buttons: Query<&TabButton>,
) {
    if ev.button != PointerButton::Primary {
        return;
    }
    let Ok(tab) = buttons.get(ev.entity) else {
        return;
    };
    if let Ok((mut transform, mut z_index)) = placements.get_mut(ev.entity) {
        transform.set_if_neq(UiTransform::default());
        z_index.set_if_neq(ZIndex::default());
    }
    if !drag.tracks(tab.id) {
        return;
    }
    // NOTE: bevy_picking triggers a release's `Click` before its `DragEnd`,
    // so the click observers still see `is_dragging` and ignore the click
    // that ends a drag. Finishing the drag on any earlier event of the
    // release would turn every drop into a selection or a close.
    if let Some((tab, index)) = drag.finish() {
        commands.trigger(RequestTabAction {
            action: TabAction::Move {
                tab,
                index: u16::try_from(index).unwrap_or(u16::MAX),
            },
        });
    }
}

/// The strip's scroll after one pointer move at `pointer_x`: a step toward
/// the edge the pointer is near, clamped to `0.0..=max_scroll`; all values
/// in logical px.
fn edge_scroll(
    pointer_x: f32,
    strip_left: f32,
    strip_width: f32,
    scroll_x: f32,
    max_scroll: f32,
) -> f32 {
    /// How close to the strip's edge the pointer must be to scroll it, in
    /// logical px.
    const EDGE_SCROLL_ZONE_PX: f32 = 24.0;
    /// How far the strip scrolls per pointer move near its edge, in logical px.
    const EDGE_SCROLL_STEP_PX: f32 = 12.0;

    if pointer_x < strip_left + EDGE_SCROLL_ZONE_PX {
        (scroll_x - EDGE_SCROLL_STEP_PX).max(0.0)
    } else if pointer_x > strip_left + strip_width - EDGE_SCROLL_ZONE_PX {
        (scroll_x + EDGE_SCROLL_STEP_PX).min(max_scroll)
    } else {
        scroll_x
    }
}

/// Stops showing a drop once the backend answered its move.
fn forget_answered_drop(mut drag: ResMut<TabDrag>, pending: Res<PendingTabMove>) {
    if pending.0.is_none() && drag.has_drop() {
        drag.forget_drop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the drop index is the tab slot under the pointer,
    /// clamped to the tab count, with the strip's scroll added.
    ///
    /// Case: the user drags a tab over the third of five 100 px tabs, then
    /// past the end, then over a strip scrolled by one tab.
    #[test]
    fn the_drop_index_is_the_slot_under_the_pointer() {
        assert_eq!(drop_index(250.0, 0.0, 0.0, 100.0, 5), 2);
        assert_eq!(drop_index(900.0, 0.0, 0.0, 100.0, 5), 4);
        assert_eq!(drop_index(-30.0, 0.0, 0.0, 100.0, 5), 0);
        assert_eq!(drop_index(50.0, 0.0, 100.0, 100.0, 5), 1);
        assert_eq!(drop_index(50.0, 0.0, 0.0, 0.0, 5), 0);
    }

    /// Asserts that a press moved less than the threshold is no drag, that
    /// a longer one is, and that finishing it returns its move once.
    ///
    /// Case: the user nudges a tab by 2 px, then drags it two slots right.
    #[test]
    fn a_drag_past_the_threshold_moves_once() {
        let mut drag = TabDrag::default();
        drag.start(TabId(1), 0, 0.0);
        drag.update(2.0, 1);
        assert!(!drag.is_dragging(TabId(1)));
        drag.update(180.0, 2);
        assert!(drag.is_dragging(TabId(1)));
        assert_eq!(drag.finish(), Some((TabId(1), 2)));
        assert!(!drag.is_dragging(TabId(1)));
        assert_eq!(drag.finish(), None);
    }

    /// Asserts that the dragged tab's visual offset follows the pointer,
    /// net of the slots the preview already moved it by and of the strip's
    /// scrolling since the press.
    ///
    /// Case: the user drags a 100 px tab 250 px right while the strip
    /// auto-scrolls by 30 px.
    #[test]
    fn the_visual_offset_follows_the_pointer_not_the_slot() {
        let mut drag = TabDrag::default();
        drag.start(TabId(1), 0, 0.0);
        drag.update(250.0, 2);
        assert_eq!(drag.visual_offset(250.0, 100.0, 0.0), 50.0);
        assert_eq!(drag.visual_offset(250.0, 100.0, 30.0), 80.0);
    }

    /// Asserts that before the threshold the tab follows the pointer from
    /// its own slot, even when the pointer is already over the next slot.
    ///
    /// Case: the user presses a tab near its right edge and moves the
    /// pointer 3 px onto the next tab.
    #[test]
    fn a_press_under_the_threshold_keeps_the_tab_in_its_slot() {
        let mut drag = TabDrag::default();
        drag.start(TabId(1), 0, 0.0);
        drag.update(3.0, 1);
        assert_eq!(drag.preview(), None);
        assert_eq!(drag.visual_offset(3.0, 100.0, 0.0), 3.0);
    }

    /// Asserts that an update would change the drag only when it crosses
    /// the threshold or moves the target slot, and never without a press.
    ///
    /// Case: the user holds a tab and wiggles the pointer inside one slot,
    /// then drags it onto the next one.
    #[test]
    fn only_the_threshold_or_a_new_slot_changes_the_drag() {
        let mut drag = TabDrag::default();
        assert!(!drag.would_change(50.0, 1));
        drag.start(TabId(1), 0, 0.0);
        assert!(!drag.would_change(2.0, 0));
        assert!(drag.would_change(5.0, 0));
        drag.update(5.0, 0);
        assert!(!drag.would_change(30.0, 0));
        assert!(!drag.would_change(1.0, 0));
        assert!(drag.would_change(130.0, 1));
    }

    /// Asserts that the strip scrolls one step toward the edge the pointer
    /// is near, never past its overflow, and stays put elsewhere.
    ///
    /// Case: the user drags a tab against the right edge of an overflowing
    /// strip, then against its left edge, then back to its middle.
    #[test]
    fn the_strip_scrolls_toward_the_edge_under_the_pointer() {
        assert_eq!(edge_scroll(590.0, 0.0, 600.0, 100.0, 300.0), 112.0);
        assert_eq!(edge_scroll(590.0, 0.0, 600.0, 295.0, 300.0), 300.0);
        assert_eq!(edge_scroll(10.0, 0.0, 600.0, 100.0, 300.0), 88.0);
        assert_eq!(edge_scroll(10.0, 0.0, 600.0, 5.0, 300.0), 0.0);
        assert_eq!(edge_scroll(300.0, 0.0, 600.0, 100.0, 300.0), 100.0);
    }

    /// Asserts that a press is tracked from its start until it finishes,
    /// whether or not it passed the threshold, and only for its own tab.
    ///
    /// Case: the user presses the first tab, nudges it, and releases it.
    #[test]
    fn a_press_is_tracked_until_it_finishes() {
        let mut drag = TabDrag::default();
        assert!(!drag.tracks(TabId(1)));
        drag.start(TabId(1), 0, 0.0);
        drag.update(1.0, 0);
        assert!(drag.tracks(TabId(1)));
        assert!(!drag.tracks(TabId(2)));
        assert_eq!(drag.finish(), None);
        assert!(!drag.tracks(TabId(1)));
    }
}
