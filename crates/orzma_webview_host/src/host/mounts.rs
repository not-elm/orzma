//! The mounted placements: each instance's current mount, pane, and size,
//! and the minting of mount ids.

use crate::boundary::MountId;
use crate::host::PaneKey;
use orzma_vt::prelude::{InstanceId, PlacementSize};
use std::collections::HashMap;

/// What a mount report changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MountChange {
    /// The placement was not mounted and now is, under this new mount.
    New(MountId),
    /// The placement stays mounted under this mount, with a new size.
    Resized(MountId),
    /// The placement stays mounted with the same size.
    Unchanged,
}

/// One mounted placement.
pub(crate) struct MountState<P> {
    mount: MountId,
    pane: P,
    size: PlacementSize,
}

impl<P: PaneKey> MountState<P> {
    /// The current mount.
    pub fn mount(&self) -> MountId {
        self.mount
    }

    /// The pane the placement sits in.
    pub fn pane(&self) -> P {
        self.pane
    }
}

/// Every mounted placement, by instance.
pub(crate) struct Mounts<P> {
    by_instance: HashMap<InstanceId, MountState<P>>,
    next: MountId,
}

impl<P> Default for Mounts<P> {
    /// No mounted placement; the first mount minted is `MountId::new(1)`.
    fn default() -> Self {
        Self {
            by_instance: HashMap::new(),
            next: MountId::new(1),
        }
    }
}

impl<P: PaneKey> Mounts<P> {
    /// Records that `instance` is mounted in `pane` over `size`: a new mount
    /// when it was not mounted, the same mount when it was.
    pub fn mount(&mut self, instance: InstanceId, pane: P, size: PlacementSize) -> MountChange {
        if let Some(state) = self.by_instance.get_mut(&instance) {
            if state.size == size {
                return MountChange::Unchanged;
            }
            state.size = size;
            return MountChange::Resized(state.mount);
        }
        let mount = self.next;
        self.next = mount.next();
        self.by_instance
            .insert(instance, MountState { mount, pane, size });
        MountChange::New(mount)
    }

    /// The mount state of `instance`, if mounted.
    pub fn get(&self, instance: InstanceId) -> Option<&MountState<P>> {
        self.by_instance.get(&instance)
    }

    /// The placement `mount` belongs to and its state, when `mount` is the
    /// placement's current mount.
    pub fn resolve(&self, mount: MountId) -> Option<(InstanceId, &MountState<P>)> {
        self.by_instance
            .iter()
            .find(|(_, state)| state.mount == mount)
            .map(|(instance, state)| (*instance, state))
    }

    /// Ends the mount of `instance`, returning its state when it was mounted.
    pub fn remove(&mut self, instance: InstanceId) -> Option<MountState<P>> {
        self.by_instance.remove(&instance)
    }

    /// Every instance mounted in `pane`, in no fixed order.
    pub fn on_pane(&self, pane: P) -> Vec<InstanceId> {
        self.by_instance
            .iter()
            .filter(|(_, state)| state.pane == pane)
            .map(|(instance, _)| *instance)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: PlacementSize = PlacementSize { rows: 10, cols: 40 };
    const LARGE: PlacementSize = PlacementSize { rows: 12, cols: 48 };

    /// Asserts that a first mount is new, a repeat with the same size
    /// changes nothing, and a repeat with another size keeps the mount.
    ///
    /// Case: the SDK re-sends its mount on every redraw and once after the
    /// window grows.
    #[test]
    fn a_mounted_placement_keeps_its_mount_across_repeats() {
        let mut mounts = Mounts::default();
        let MountChange::New(mount) = mounts.mount(InstanceId(1), 1_u32, SMALL) else {
            panic!("the first mount is new");
        };
        assert_eq!(
            mounts.mount(InstanceId(1), 1, SMALL),
            MountChange::Unchanged
        );
        assert_eq!(
            mounts.mount(InstanceId(1), 1, LARGE),
            MountChange::Resized(mount)
        );
    }

    /// Asserts that mounting again after the mount ended mints a new,
    /// larger mount id.
    ///
    /// Case: the alternate screen exits and the program mounts the same
    /// placement again.
    #[test]
    fn a_remount_after_an_unmount_mints_a_new_mount() {
        let mut mounts = Mounts::default();
        let MountChange::New(first) = mounts.mount(InstanceId(1), 1_u32, SMALL) else {
            panic!("the first mount is new");
        };
        assert!(mounts.remove(InstanceId(1)).is_some());
        let MountChange::New(second) = mounts.mount(InstanceId(1), 1, SMALL) else {
            panic!("the remount is new");
        };
        assert!(second > first);
    }

    /// Asserts that `on_pane` lists exactly the instances mounted in that
    /// pane.
    ///
    /// Case: a program unmounts everything in its pane while another pane
    /// keeps its page.
    #[test]
    fn on_pane_lists_only_that_panes_mounts() {
        let mut mounts = Mounts::default();
        let _ = mounts.mount(InstanceId(1), 1_u32, SMALL);
        let _ = mounts.mount(InstanceId(2), 1, SMALL);
        let _ = mounts.mount(InstanceId(3), 2, SMALL);
        let mut on_one = mounts.on_pane(1);
        on_one.sort();
        assert_eq!(on_one, [InstanceId(1), InstanceId(2)]);
    }

    /// Asserts that a mount id resolves to its placement only while it is
    /// that placement's current mount.
    ///
    /// Case: the GUI reports a click on a page that was just unmounted and
    /// mounted again.
    #[test]
    fn a_mount_resolves_only_while_current() {
        let mut mounts = Mounts::default();
        let MountChange::New(first) = mounts.mount(InstanceId(1), 1_u32, SMALL) else {
            panic!("the first mount is new");
        };
        assert_eq!(
            mounts.resolve(first).map(|(instance, _)| instance),
            Some(InstanceId(1))
        );
        let _ = mounts.remove(InstanceId(1));
        let MountChange::New(second) = mounts.mount(InstanceId(1), 1, SMALL) else {
            panic!("the remount is new");
        };
        assert!(mounts.resolve(first).is_none());
        assert!(mounts.resolve(second).is_some());
    }
}
