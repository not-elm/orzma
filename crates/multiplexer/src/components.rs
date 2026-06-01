//! Bevy `Component` definitions for the multiplexer. Every entity that
//! plays a role in the Session / Pane / Activity hierarchy carries a
//! marker component (`SessionMarker` / `PaneMarker` / `ActivityMarker`)
//! plus the state components relevant to its role.
//!
//! Display names use Bevy's built-in `Name` (`bevy::prelude::Name`); no
//! `SessionName` / `PaneName` / `ActivityName` component exists. See the
//! design doc §3 "Naming" for why and for the `With<SessionMarker>`
//! filter discipline that follows.

use crate::cells::{CellId, LayoutCellState};
use bevy::prelude::*;
use std::path::PathBuf;

/// Zero-sized marker on every Session entity. Used as the `With<>` filter
/// in queries that want to scope to Sessions, and as the trigger target
/// for the `On<Remove, SessionMarker>` lifecycle hook.
#[derive(Component, Default, Debug)]
pub struct SessionMarker;

/// Zero-sized marker on every Pane entity.
#[derive(Component, Default, Debug)]
pub struct PaneMarker;

/// Zero-sized marker on every Activity entity.
#[derive(Component, Default, Debug)]
pub struct ActivityMarker;

/// Layout cell state plus the session's `root` cell id, owned together
/// because every consumer needs both. `Default` returns a freshly-empty
/// `LayoutCellState` and a `CellId(0)` placeholder root that must be
/// overwritten by the spawn site (`MultiplexerCommands::create_session`).
#[derive(Component, Debug, Default, Clone)]
pub struct LayoutCells {
    /// The BSP cell tree for this session.
    pub cells: LayoutCellState,
    /// The root `CellId` of the cell tree.
    pub root: CellId,
}

impl LayoutCells {
    /// Construct a `LayoutCells` from a freshly-spawned bootstrap pane:
    /// builds a `LayoutCellState`, mints the root, and stashes the root
    /// id together so callers do not need to track it separately.
    pub fn new_session_layout(pane: Entity) -> Self {
        let mut cells = LayoutCellState::default();
        let (root, _pane_cell) = cells.new_session_layout(pane);
        Self { cells, root }
    }
}

/// The currently focused Pane entity within a Session. `Changed<ActivePane>`
/// is the signal for the terminal-focus and IME-target-switch systems.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivePane(pub Entity);

/// Cached cell-grid dimensions for a Session, set by the renderer.
/// Absent until the first measurement (represented as the component
/// being absent, not as `Option`-inside-component).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionDimensions {
    /// Number of columns in the session grid.
    pub cols: u16,
    /// Number of rows in the session grid.
    pub rows: u16,
}

/// Marker that exactly one Session entity carries: the one currently
/// rendered in the primary OS window. Moving the marker swaps which
/// Session is shown. Identical semantics to the old GUI component of
/// the same name (moved here from `src/session_entity.rs`).
#[derive(Component, Default, Debug)]
pub struct AttachedSession;

/// Per-Session pointer to the Entity that hosts the Session's UI subtree
/// root. The subtree root is a `Node`; when the Session is attached, the
/// subtree's `ChildOf` is `SessionUiRoot`. When parked, it is the Session
/// entity itself (walker-skipped). The component is bundled with the
/// other Session-side components, but the UI layer is responsible for
/// spawning the subtree node and inserting the pointer.
#[derive(Component, Debug, Clone, Copy)]
pub struct SessionUiSubtree(pub Entity);

/// The currently focused Activity entity within a Pane.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveActivity(pub Entity);

/// Per-Pane copy-mode state. `Off` means copy mode is inactive on this
/// Pane. `Active` is a zero-variant marker: the vi cursor and active
/// selection are owned by `alacritty_terminal::Term` inside
/// `TerminalHandle`; no coordinates need to be duplicated here.
/// `Changed<CopyMode>` is the signal for focus and indicator systems.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub enum CopyMode {
    #[default]
    Off,
    Active,
}

/// Per-Pane cached cell-grid dimensions, derived from the layout. Set
/// by the resize-terminal system.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneDimensions {
    /// Number of columns in the pane grid.
    pub cols: u16,
    /// Number of rows in the pane grid.
    pub rows: u16,
}

/// The SDK-side activity id (handler-registration key) for an extension
/// activity. Stamped by the control bridge so the handlers bridge can address
/// `{aid, frame}` envelopes to the right handler set.
#[derive(Component, Debug, Clone)]
pub struct ExtensionActivityAid(pub String);

/// The name of the extension that owns an extension activity. Stamped by the
/// control bridge from the split request so the renderer can resolve the
/// webview URL host and the handlers socket per extension.
#[derive(Component, Debug, Clone)]
pub struct OwningExtension(pub String);

/// Activity kind discriminator. Ported from the old crate's
/// `ActivityKind` enum; field types preserved.
#[derive(Component, Debug, Clone)]
pub enum ActivityKind {
    /// A PTY-backed terminal activity.
    Terminal,
    /// An extension activity served from a Node process over a UDS.
    Extension {
        /// HTML entry path (relative to the extension dir) the webview loads.
        entry: PathBuf,
    },
    /// An embedded Chromium browser activity.
    Browser {
        /// URL to navigate to on creation, or `None` to use the browser default.
        initial_url: Option<String>,
        /// Storage profile for this browser instance.
        profile: BrowserProfile,
    },
}

/// Storage profile for a Browser Activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserProfile {
    /// A named persistent profile stored under the given name.
    Named {
        /// Profile directory name (relative to the browser data root).
        name: String,
    },
    /// A temporary profile that is discarded when the activity closes.
    Incognito,
}

impl Default for BrowserProfile {
    fn default() -> Self {
        BrowserProfile::Named {
            name: "default".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;
    use bevy::prelude::With;

    #[test]
    fn markers_are_components_and_query_filterable() {
        let mut world = World::new();
        let session = world.spawn(SessionMarker).id();
        let pane = world.spawn(PaneMarker).id();
        let activity = world.spawn(ActivityMarker).id();

        let mut q = world.query_filtered::<Entity, With<SessionMarker>>();
        let sessions: Vec<_> = q.iter(&world).collect();
        assert_eq!(sessions, vec![session]);

        let mut q = world.query_filtered::<Entity, With<PaneMarker>>();
        let panes: Vec<_> = q.iter(&world).collect();
        assert_eq!(panes, vec![pane]);

        let mut q = world.query_filtered::<Entity, With<ActivityMarker>>();
        let activities: Vec<_> = q.iter(&world).collect();
        assert_eq!(activities, vec![activity]);
    }

    #[test]
    fn copy_mode_default_is_off() {
        assert_eq!(CopyMode::default(), CopyMode::Off);
    }

    #[test]
    fn browser_profile_default_is_named_default() {
        assert!(matches!(
            BrowserProfile::default(),
            BrowserProfile::Named { name } if name == "default",
        ));
    }
}
