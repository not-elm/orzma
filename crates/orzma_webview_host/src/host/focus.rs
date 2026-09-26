//! Which mount holds webview keyboard focus, and where changes of it are
//! reported.

use crate::boundary::{HandleId, MountId};
use crate::control_socket::ConnectionId;
use crate::host::PaneKey;
use orzma_vt::prelude::InstanceId;

/// Where one mount's focus changes are reported: the mount, its placement
/// and registration, the connection that owns it, and its pane.
///
/// The route is kept while the mount holds focus, so the `false` push still
/// reaches the program after its registration is released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FocusRoute<P> {
    mount: MountId,
    instance: InstanceId,
    handle: HandleId,
    connection: ConnectionId,
    pane: P,
}

impl<P: PaneKey> FocusRoute<P> {
    /// The route of `mount`, a mount of `instance` under `handle`, owned by
    /// `connection`, in `pane`.
    pub fn new(
        mount: MountId,
        instance: InstanceId,
        handle: HandleId,
        connection: ConnectionId,
        pane: P,
    ) -> Self {
        Self {
            mount,
            instance,
            handle,
            connection,
            pane,
        }
    }

    /// The focused mount.
    pub fn mount(&self) -> MountId {
        self.mount
    }

    /// The placement the mount belongs to.
    pub fn instance(&self) -> InstanceId {
        self.instance
    }

    /// The registration the placement belongs to.
    pub fn handle(&self) -> &HandleId {
        &self.handle
    }

    /// The connection pushes go to.
    pub fn connection(&self) -> ConnectionId {
        self.connection
    }

    /// The pane the mount sits in.
    pub fn pane(&self) -> P {
        self.pane
    }
}

/// What moving focus to a route changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FocusTransition<P> {
    /// The route already held focus.
    Unchanged,
    /// Focus moved to the route; `lost` held it before, if anything did.
    Moved {
        /// The route that lost focus.
        lost: Option<FocusRoute<P>>,
    },
}

/// The route of the mount holding webview keyboard focus, if any.
pub(crate) struct FocusState<P> {
    current: Option<FocusRoute<P>>,
}

impl<P: PaneKey> FocusState<P> {
    /// No mount holds focus.
    pub fn new() -> Self {
        Self { current: None }
    }

    /// The route holding focus.
    pub fn current(&self) -> Option<&FocusRoute<P>> {
        self.current.as_ref()
    }

    /// The mount holding focus.
    pub fn focused_mount(&self) -> Option<MountId> {
        self.current.as_ref().map(FocusRoute::mount)
    }

    /// Moves focus to `route`.
    pub fn set(&mut self, route: FocusRoute<P>) -> FocusTransition<P> {
        if self.current.as_ref() == Some(&route) {
            return FocusTransition::Unchanged;
        }
        FocusTransition::Moved {
            lost: self.current.replace(route),
        }
    }

    /// Releases focus, returning the route that held it.
    pub fn clear(&mut self) -> Option<FocusRoute<P>> {
        self.current.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(mount: u64) -> FocusRoute<u32> {
        FocusRoute::new(
            MountId::new(mount),
            InstanceId(u128::from(mount)),
            HandleId::from("h"),
            ConnectionId::new(1),
            1,
        )
    }

    /// Asserts that focusing a route reports the route that lost focus, and
    /// that focusing the holder again reports no change.
    ///
    /// Case: the user clicks one page, then another, then the second again.
    #[test]
    fn moving_focus_reports_the_route_that_lost_it() {
        let mut focus = FocusState::new();
        assert_eq!(focus.set(route(1)), FocusTransition::Moved { lost: None });
        assert_eq!(
            focus.set(route(2)),
            FocusTransition::Moved {
                lost: Some(route(1))
            }
        );
        assert_eq!(focus.set(route(2)), FocusTransition::Unchanged);
        assert_eq!(focus.focused_mount(), Some(MountId::new(2)));
    }

    /// Asserts that clearing focus returns the holder once, then nothing.
    ///
    /// Case: the user presses the release-focus shortcut twice.
    #[test]
    fn clearing_focus_returns_the_holder_once() {
        let mut focus = FocusState::new();
        let _ = focus.set(route(1));
        assert_eq!(focus.clear(), Some(route(1)));
        assert_eq!(focus.clear(), None);
        assert_eq!(focus.focused_mount(), None);
    }
}
