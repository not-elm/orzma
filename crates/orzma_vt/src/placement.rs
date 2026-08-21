//! Webview placement table: id minting, anchor tracking, and viewport
//! projection.
//!
//! [`PlacementStore`] is a side table keyed by the grid line a mount
//! anchored to, never a cell variant, so text writes and reflow cannot
//! corrupt a placement. It converts to viewport coordinates only at
//! emit time.

use crate::device::ActiveScreen;
use crate::schema::{
    DisplayOffset, GridColumn, GridSize, PlacementId, ProjectedPlacement, ScreenKind,
};
use crate::screen::grid::LineId;

/// The placement table: minted ids, line anchors, and occupancy spans.
// TODO: Carry the per-line occupancy spans and the anchor bookkeeping
// `HistoryEvent` drives.
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

    /// Projects every placement on `active_screen` into viewport
    /// coordinates — the complete list, not a diff.
    ///
    /// # Invariants
    ///
    /// Projection reads; it never evicts, repairs an anchor, or
    /// refreshes a cache. Those belong to `HistoryEvent` handling,
    /// which runs while damage can still be staged — a mutation here
    /// would land after the ledger was drained and reach no frame.
    // TODO: Return the placements anchored on `active_screen` once the
    // table exists. An empty list is the honest answer while nothing can
    // be mounted.
    pub fn project(
        &self,
        _active_screen: ScreenKind,
        _offset: DisplayOffset,
        _size: GridSize,
    ) -> Vec<ProjectedPlacement> {
        Vec::new()
    }

    /// Registers a mount at the cursor and mints its id; `None` when
    /// policy rejects it.
    ///
    /// A live `(view_id, instance_id)` is superseded silently — the old
    /// placement is dropped without being reported as evicted, because
    /// the host re-points the same entity at the successor and would
    /// despawn it if the superseded id were named.
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
            .retain(|p| p.view_id != view_id || p.instance_id != instance_id);
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
    pub fn unmount(&mut self, view_id: Option<&str>, instance_id: Option<&str>) -> bool {
        let before = self.placements.len();
        self.placements.retain(|p| match (view_id, instance_id) {
            (None, _) => false,
            (Some(view), None) => p.view_id != view,
            (Some(view), Some(instance)) => {
                p.view_id != view || p.instance_id.as_deref() != Some(instance)
            }
        });
        before != self.placements.len()
    }

    /// Number of live placements, across both screens.
    fn len(&self) -> usize {
        self.placements.len()
    }

    /// Whether the table holds no placements.
    fn is_empty(&self) -> bool {
        self.placements.is_empty()
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
}
