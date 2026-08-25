//! Webview placement table: id minting, anchor tracking, and viewport
//! projection.
//!
//! [`PlacementStore`] is a side table keyed by the grid row a mount
//! anchored to, never a cell variant, so text writes and reflow cannot
//! corrupt a placement. It converts to viewport coordinates only at
//! emit time.

use crate::device::ActiveScreen;
use crate::schema::{GridColumn, PlacementId, ProjectedPlacement, ScreenKind};
use crate::screen::grid::LineId;

/// The placement table: minted ids, line anchors, and occupancy spans.
// TODO: Carry the per-line occupancy spans a mount reserves. Anchors
// already resolve per emit as `LineId`s, so only that reservation
// bookkeeping remains unimplemented.
pub(crate) struct PlacementStore {
    next_id: PlacementId,
    placements: Vec<Placement>,
}

impl PlacementStore {
    /// Builds an empty store whose first minted id is unused.
    pub fn new() -> Self {
        Self {
            next_id: PlacementId(0),
            placements: Vec::new(),
        }
    }

    /// Projects every placement on the active screen into viewport
    /// coordinates, into a caller-owned buffer — the complete list, not
    /// a diff.
    ///
    /// # Invariants
    ///
    /// Projection reads; it never evicts, repairs an anchor, or refreshes a
    /// cache. Those belong to [`Self::evict_lost_anchors`], which runs while
    /// damage can still be staged — a mutation here would land after the
    /// ledger was drained and reach no frame.
    ///
    /// A placement whose anchor has left the ring is omitted rather than
    /// removed, so it stays invisible but alive until the next eviction
    /// sweep. Rows outside the viewport are not culled: a negative row
    /// passes through for the renderer to clip.
    pub fn project_into(&self, out: &mut Vec<ProjectedPlacement>, active: ActiveScreen<'_>) {
        out.extend(
            self.placements
                .iter()
                .filter(|p| p.screen == active.kind())
                .filter_map(|p| {
                    Some(ProjectedPlacement {
                        id: p.id,
                        viewport_row: active.viewport_row_of(p.anchor)?,
                        col: p.col,
                        rows: p.rows,
                        cols: p.cols,
                    })
                }),
        );
    }

    /// Projects into a fresh `Vec`; see [`Self::project_into`].
    #[cfg(test)]
    pub fn project(&self, active: ActiveScreen<'_>) -> Vec<ProjectedPlacement> {
        let mut out = Vec::new();
        self.project_into(&mut out, active);
        out
    }

    /// Registers a mount at the cursor and mints its id; `None` when
    /// policy rejects it.
    ///
    /// A live `(view_id, instance_id)` is superseded silently — the old
    /// placement is dropped without being reported as evicted, because
    /// the host re-points the same entity at the successor and would
    /// despawn it if the superseded id were named.
    ///
    /// The caller raises the chunk liveness flag when this returns `Some`, so
    /// the placement list a mount changes always reaches the next frame.
    ///
    /// # Invariants
    ///
    /// The replacement runs before the cap check: a re-mount frees the
    /// slot it takes, so it must succeed even at the limit.
    pub fn mount(
        &mut self,
        active: ActiveScreen<'_>,
        rows: u16,
        cols: u16,
        view_id: String,
        instance_id: Option<String>,
    ) -> Option<PlacementId> {
        self.placements
            .retain(|p| !p.addressed_by(&view_id, instance_id.as_deref()));
        if MAX_PLACEMENTS <= self.placements.len() {
            return None;
        }
        let id = self.next_id;
        self.next_id = PlacementId(
            id.0.checked_add(1)
                .expect("a session cannot mint u64::MAX placements"),
        );
        self.placements.push(Placement {
            id,
            screen: active.kind(),
            anchor: active.cursor_line_id(),
            col: active.cursor_column(),
            rows,
            cols,
            view_id,
            instance_id,
        });
        Some(id)
    }

    /// Removes the placements the client asked to unmount; returns
    /// whether anything went.
    ///
    /// Client-initiated, so nothing is reported as evicted — the host
    /// acts on the verb itself.
    ///
    /// The caller raises the chunk liveness flag when this returns `true`, so a
    /// change to the placement list always reaches the next frame.
    pub fn unmount(&mut self, view_id: Option<&str>, instance_id: Option<&str>) -> bool {
        let before = self.placements.len();
        self.placements.retain(|p| match (view_id, instance_id) {
            (None, _) => false,
            (Some(view), None) => p.view_id != view,
            (Some(view), Some(instance)) => !p.addressed_by(view, Some(instance)),
        });
        before != self.placements.len()
    }

    /// Drops the placements whose anchor row left the grid ring and
    /// returns their ids for `VtSignal::WebviewEvicted`.
    ///
    /// Only the active screen can be checked, which is sound while the
    /// inactive grid never scrolls. Reflow breaks that and will have to
    /// sweep both.
    ///
    /// The caller raises the chunk liveness flag when the returned list is
    /// non-empty, so an eviction always reaches the next frame.
    pub fn evict_lost_anchors(&mut self, active: ActiveScreen<'_>) -> Vec<PlacementId> {
        if self.is_empty() {
            return Vec::new();
        }
        self.evict_where(|p| {
            p.screen == active.kind() && active.viewport_row_of(p.anchor).is_none()
        })
    }

    /// Applies an alternate-screen flip, tearing down the placements the
    /// abandoned alternate screen owned.
    ///
    /// Primary placements are hidden while the alternate screen is shown,
    /// not destroyed. This operation stages no damage of its own: the
    /// flip itself must stage `Damage::Full`, which carries the changed
    /// list.
    pub fn switch_screen(&mut self, to: ScreenKind) -> Vec<PlacementId> {
        if to == ScreenKind::Alternate {
            return Vec::new();
        }
        self.evict_where(|p| p.screen != ScreenKind::Primary)
    }

    /// Number of live placements, across both screens.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.placements.len()
    }

    /// Whether the table holds no placements.
    fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }

    /// Drops every placement `should_evict` accepts and returns their ids.
    fn evict_where(
        &mut self,
        mut should_evict: impl FnMut(&Placement) -> bool,
    ) -> Vec<PlacementId> {
        let mut evicted = Vec::new();
        self.placements.retain(|p| {
            if should_evict(p) {
                evicted.push(p.id);
                return false;
            }
            true
        });
        evicted
    }
}

/// One mounted webview: its anchor, reserved footprint, and address.
struct Placement {
    id: PlacementId,
    screen: ScreenKind,
    anchor: LineId,
    col: GridColumn,
    rows: u16,
    cols: u16,
    view_id: String,
    instance_id: Option<String>,
}

impl Placement {
    /// Whether this placement is the one `(view_id, instance_id)` addresses.
    fn addressed_by(&self, view_id: &str, instance_id: Option<&str>) -> bool {
        self.view_id == view_id && self.instance_id.as_deref() == instance_id
    }
}

/// Upper bound on live placements per terminal, across both screens.
///
/// It matches the renderer's overlay slot count, so a mount the VT
/// accepts is always one the host can place. The two are not mirrors:
/// the host allocates slots per terminal among live children, while this
/// cap counts both screens, so it is strictly the stricter of the two.
const MAX_PLACEMENTS: usize = 12;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceState;
    use crate::schema::GridSize;

    fn device() -> DeviceState {
        DeviceState::new(GridSize { cols: 8, rows: 3 }, 10)
    }

    fn mount(store: &mut PlacementStore, device: &DeviceState, view: &str) -> Option<PlacementId> {
        store.mount(device.active_screen(), 2, 4, view.to_string(), None)
    }

    /// Asserts that ids are minted in ascending order and never repeat.
    ///
    /// Case: a program mounts two separate views and the host has to
    /// address each one for its lifetime.
    #[test]
    fn ids_are_minted_ascending_and_never_repeat() {
        let device = device();
        let mut store = PlacementStore::new();
        let first = mount(&mut store, &device, "a").expect("first mount accepted");
        let second = mount(&mut store, &device, "b").expect("second mount accepted");
        assert!(first < second);
        store.unmount(None, None);
        let third = mount(&mut store, &device, "c").expect("third mount accepted");
        assert!(second < third);
    }

    /// Asserts that a mount past the table's cap is rejected.
    ///
    /// Case: a runaway program keeps mounting views without ever
    /// unmounting them.
    #[test]
    fn a_mount_past_the_cap_is_rejected() {
        let device = device();
        let mut store = PlacementStore::new();
        for index in 0..MAX_PLACEMENTS {
            assert!(
                mount(&mut store, &device, &format!("v{index}")).is_some(),
                "mount {index} should fit"
            );
        }
        assert_eq!(mount(&mut store, &device, "overflow"), None);
    }

    /// Asserts that re-mounting a live address succeeds at the cap,
    /// because the replacement frees the slot it takes.
    ///
    /// The order matters: checking the cap first would reject a program
    /// that merely keeps refreshing the same view.
    ///
    /// Case: a dashboard re-mounts its single view on every redraw while
    /// eleven other views are already mounted.
    #[test]
    fn re_mounting_a_live_address_succeeds_at_the_cap() {
        let device = device();
        let mut store = PlacementStore::new();
        for index in 0..MAX_PLACEMENTS {
            mount(&mut store, &device, &format!("v{index}")).expect("mount fits");
        }
        assert!(mount(&mut store, &device, "v0").is_some());
    }

    /// Asserts that unmount honours its three scopes.
    ///
    /// Case: a program tears down one instance, then a whole view, then
    /// everything it had mounted.
    #[test]
    fn unmount_honours_its_three_scopes() {
        let device = device();
        let mut store = PlacementStore::new();
        store
            .mount(
                device.active_screen(),
                2,
                4,
                "memo".into(),
                Some("a".into()),
            )
            .expect("mount accepted");
        store
            .mount(
                device.active_screen(),
                2,
                4,
                "memo".into(),
                Some("b".into()),
            )
            .expect("mount accepted");
        mount(&mut store, &device, "other").expect("mount accepted");

        assert!(store.unmount(Some("memo"), Some("a")));
        assert_eq!(store.len(), 2);
        assert!(store.unmount(Some("memo"), None));
        assert_eq!(store.len(), 1);
        assert!(store.unmount(None, None));
        assert_eq!(store.len(), 0);
    }

    /// Asserts that an unmount matching nothing reports that it changed
    /// nothing.
    ///
    /// Case: a program unmounts a handle it already tore down, so no
    /// frame needs to be forced.
    #[test]
    fn an_unmount_matching_nothing_reports_no_change() {
        let mut store = PlacementStore::new();
        assert!(!store.unmount(Some("absent"), None));
    }

    /// Asserts that a placement's projected row follows its text as the
    /// screen scrolls.
    ///
    /// Case: a webview is mounted mid-screen and the shell keeps printing
    /// below it.
    #[test]
    fn a_projected_row_follows_its_text_as_content_scrolls() {
        let mut device = device();
        let mut store = PlacementStore::new();
        let id = mount(&mut store, &device, "memo").expect("mount accepted");
        assert_eq!(store.project(device.active_screen())[0].viewport_row, 0);

        for _ in 0..3 {
            device.active_mut().line_feed();
        }
        let projected = store.project(device.active_screen());
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, id);
        assert_eq!(projected[0].viewport_row, -1);
    }

    /// Asserts that a placement mounted on one screen is omitted while the
    /// other is shown, and returns when it is active again.
    ///
    /// Case: a full-screen editor opens over a shell that has a webview
    /// mounted, and the user quits back to the shell.
    #[test]
    fn a_placement_is_hidden_while_the_other_screen_is_active() {
        let mut device = device();
        let mut store = PlacementStore::new();
        mount(&mut store, &device, "memo").expect("mount accepted");

        device.set_active_screen_for_test(ScreenKind::Alternate);
        assert!(store.project(device.active_screen()).is_empty());

        device.set_active_screen_for_test(ScreenKind::Primary);
        assert_eq!(store.project(device.active_screen()).len(), 1);
    }

    /// Asserts that leaving the alternate screen tears down the placements
    /// mounted on it.
    ///
    /// Case: a full-screen application that mounted a panel exits, and the
    /// panel must not outlive the screen it was drawn on.
    #[test]
    fn leaving_the_alternate_screen_evicts_its_placements() {
        let mut device = device();
        let mut store = PlacementStore::new();
        let primary = mount(&mut store, &device, "shell").expect("mount accepted");

        device.set_active_screen_for_test(ScreenKind::Alternate);
        let alternate = mount(&mut store, &device, "panel").expect("mount accepted");

        assert_eq!(store.switch_screen(ScreenKind::Primary), vec![alternate]);
        device.set_active_screen_for_test(ScreenKind::Primary);
        let projected = store.project(device.active_screen());
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, primary);
    }

    /// Asserts that a placement whose anchor row left the ring is evicted
    /// and named.
    ///
    /// Case: the scrollback reaches its cap and trims away the row a
    /// webview was anchored to.
    #[test]
    fn a_placement_whose_anchor_left_the_ring_is_evicted() {
        let mut device = DeviceState::new(GridSize { cols: 8, rows: 3 }, 1);
        let mut store = PlacementStore::new();
        let id = mount(&mut store, &device, "memo").expect("mount accepted");
        for _ in 0..4 {
            device.active_mut().line_feed();
        }
        assert!(store.project(device.active_screen()).is_empty());
        assert_eq!(store.evict_lost_anchors(device.active_screen()), vec![id]);
        assert_eq!(store.len(), 0);
    }

    /// Asserts that an empty table evicts nothing.
    ///
    /// Case: an ordinary shell session with no webview mounted scrolls its
    /// output, which runs this check on every linefeed.
    #[test]
    fn an_empty_table_evicts_nothing() {
        let device = device();
        let mut store = PlacementStore::new();
        assert!(store.evict_lost_anchors(device.active_screen()).is_empty());
    }

    /// Asserts that a sweep skips a placement mounted on the other screen
    /// instead of resolving its anchor against the active screen's grid.
    ///
    /// Case: a full-screen editor is open over a shell with a webview
    /// mounted, and a sweep runs while the editor's alternate screen is
    /// active.
    #[test]
    fn a_sweep_leaves_the_other_screens_placement_alone() {
        let mut device = device();
        let mut store = PlacementStore::new();
        let primary = mount(&mut store, &device, "memo").expect("mount accepted");

        device.set_active_screen_for_test(ScreenKind::Alternate);
        assert!(store.evict_lost_anchors(device.active_screen()).is_empty());

        device.set_active_screen_for_test(ScreenKind::Primary);
        let projected = store.project(device.active_screen());
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, primary);
    }

    /// Asserts that a placement inside the scrolled region follows its row
    /// down.
    ///
    /// Case: a webview is mounted beside a line of output and a
    /// full-screen application scrolls the screen backwards under it.
    #[test]
    fn a_reverse_scroll_moves_a_placement_down_with_its_row() {
        let mut device = device();
        let mut store = PlacementStore::new();
        let id = mount(&mut store, &device, "memo").expect("mount accepted");
        assert_eq!(store.project(device.active_screen())[0].viewport_row, 0);

        device.active_mut().reverse_index();

        let projected = store.project(device.active_screen());
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, id);
        assert_eq!(projected[0].viewport_row, 1);
    }

    /// Asserts that a placement on the row a reverse scroll discards stops
    /// projecting, and that the next sweep names it.
    ///
    /// The agreed policy leaves the removal to `evict_lost_anchors` rather
    /// than doing it inside the scroll: projection reads and never
    /// mutates, so a discarded placement goes invisible at once and is
    /// reclaimed when the sweep runs.
    ///
    /// Case: a webview sits on the last row of the screen and a
    /// full-screen application scrolls backwards, pushing that row off the
    /// bottom.
    #[test]
    fn a_placement_on_the_discarded_row_stops_projecting_and_is_swept() {
        let mut device = device();
        let mut store = PlacementStore::new();
        device.active_mut().line_feed();
        device.active_mut().line_feed();
        let id = mount(&mut store, &device, "memo").expect("mount accepted");
        assert_eq!(store.project(device.active_screen())[0].viewport_row, 2);

        device.active_mut().reverse_index();
        device.active_mut().reverse_index();
        device.active_mut().reverse_index();

        assert!(store.project(device.active_screen()).is_empty());
        assert_eq!(store.evict_lost_anchors(device.active_screen()), vec![id]);
        assert_eq!(store.len(), 0);
    }

    /// Asserts that an anchor still resolves once the history holds ids
    /// that are no longer ascending.
    ///
    /// Case: a full-screen application scrolls backwards — minting a row
    /// with a high id above older rows — and then output pushes that row
    /// into history ahead of the ones it was inserted above.
    #[test]
    fn an_anchor_still_resolves_once_the_history_ids_are_unordered() {
        let mut device = device();
        let mut store = PlacementStore::new();
        let id = mount(&mut store, &device, "memo").expect("mount accepted");

        device.active_mut().reverse_index();
        for _ in 0..3 {
            device.active_mut().line_feed();
        }

        let projected = store.project(device.active_screen());
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, id);
        assert_eq!(projected[0].viewport_row, 0);
    }
}
