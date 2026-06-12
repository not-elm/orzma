//! Bevy `Component` definitions for the multiplexer. Every entity that
//! plays a role in the Workspace / Pane / Surface hierarchy carries a
//! marker component (`WorkspaceMarker` / `PaneMarker` / `SurfaceMarker`)
//! plus the state components relevant to its role.
//!
//! Display names use Bevy's built-in `Name` (`bevy::prelude::Name`); no
//! `WorkspaceName` / `PaneName` / `SurfaceName` component exists. See the
//! design doc §3 "Naming" for why and for the `With<WorkspaceMarker>`
//! filter discipline that follows.

use crate::layout::SplitOrientation;
use bevy::prelude::*;
use std::path::PathBuf;

/// Zero-sized marker on every Workspace entity. Used as the `With<>` filter
/// in queries that want to scope to Workspaces, and as the trigger target
/// for the `On<Remove, WorkspaceMarker>` lifecycle hook.
#[derive(Component, Default, Debug)]
pub struct WorkspaceMarker;

/// Zero-sized marker on every Pane entity.
#[derive(Component, Default, Debug)]
pub struct PaneMarker;

/// Zero-payload-plus-orientation marker on every Split entity. The split's
/// two children carry the relative weights as `flex_grow`; this only records
/// the axis. The split entity also carries a flex-container `Node` whose
/// `flex_direction` matches `orientation`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitNode {
    /// Axis of this split.
    pub orientation: SplitOrientation,
}

/// Back-pointer from a Pane to its owning Workspace. Required because a Pane
/// is `ChildOf` its `Split` (or the layout-root node) — never the Workspace
/// directly — and the layout-root node reparents between the Workspace
/// (parked) and `WorkspaceUiRoot` (attached), so an ancestor walk cannot
/// reliably reach the Workspace.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwningWorkspace(pub Entity);

/// Zero-sized marker on every Surface entity.
#[derive(Component, Default, Debug)]
pub struct SurfaceMarker;

/// Logical Pane→Surface ownership (one direction). A Surface points at its
/// owning Pane regardless of where the surface currently sits in the layout
/// `ChildOf` tree (slotted under the pane's surface-slot, or parked under the
/// non-Node Workspace). Paired with [`Surfaces`].
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[relationship(relationship_target = Surfaces)]
pub struct SurfaceOf(#[entities] pub Entity);

/// The Surfaces a Pane owns (auto-maintained reverse collection of
/// [`SurfaceOf`]). `linked_spawn`: despawning the Pane cascade-despawns every
/// owned Surface — including inactive ones parked under the Workspace, which
/// are NOT `ChildOf(pane)` and would otherwise leak.
#[derive(Component, Debug, Default)]
#[relationship_target(relationship = SurfaceOf, linked_spawn)]
pub struct Surfaces(Vec<Entity>);

/// The currently focused Pane entity within a Workspace. `Changed<ActivePane>`
/// is the signal for the terminal-focus and IME-target-switch systems.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivePane(pub Entity);

/// Cached cell-grid dimensions for a Workspace, set by the renderer.
/// Absent until the first measurement (represented as the component
/// being absent, not as `Option`-inside-component).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceDimensions {
    /// Number of columns in the workspace grid.
    pub cols: u16,
    /// Number of rows in the workspace grid.
    pub rows: u16,
}

/// Marker that exactly one Workspace entity carries: the one currently
/// rendered in the primary OS window. Moving the marker swaps which
/// Workspace is shown. Identical semantics to the old GUI component of
/// the same name (moved here from `src/workspace_entity.rs`).
#[derive(Component, Default, Debug)]
pub struct AttachedWorkspace;

/// Per-Workspace pointer to the Entity that hosts the Workspace's UI subtree
/// root. The subtree root is a `Node`; when the Workspace is attached, the
/// subtree's `ChildOf` is `WorkspaceUiRoot`. When parked, it is the Workspace
/// entity itself (walker-skipped). The subtree node is spawned and this
/// pointer inserted by `MultiplexerCommands::spawn_attached_workspace`.
#[derive(Component, Debug, Clone, Copy)]
pub struct WorkspaceUiSubtree(pub Entity);

/// Per-Workspace monotonic creation-order index, set at spawn time from
/// `WorkspaceNameCounter`. Stable sort key for UIs that list workspaces in
/// creation order (status bar, focus cycling); `Entity` ordering is
/// unreliable because deferred command queues do not guarantee monotonic
/// indices across multiple `Commands` instances.
#[derive(Component, Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct WorkspaceCreatedAt(pub u32);

/// The currently focused Surface entity within a Pane.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveSurface(pub Entity);

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

/// The SDK-side surface id (handler-registration key) for an extension
/// surface. Stamped by the control bridge so the handlers bridge can address
/// `{surface_id, frame}` envelopes to the right handler set.
#[derive(Component, Debug, Clone)]
pub struct ExtensionSurfaceId(pub String);

/// The name of the extension that owns an extension surface. Stamped by the
/// control bridge from the split request so the renderer can resolve the
/// webview URL host and the handlers socket per extension.
#[derive(Component, Debug, Clone)]
pub struct OwningExtension(pub String);

/// Surface kind discriminator. Ported from the old crate's
/// `SurfaceKind` enum; field types preserved.
#[derive(Component, Debug, Clone)]
pub enum SurfaceKind {
    /// A PTY-backed terminal surface.
    Terminal,
    /// An extension surface served from a Node process over a UDS.
    Extension {
        /// HTML entry path (relative to the extension dir) the webview loads.
        entry: PathBuf,
    },
}

/// The current working directory of a surface. For terminal surfaces this is
/// kept live by OSC 7; other kinds carry their creation-time value. Absence
/// means "unknown" — the terminal spawner falls back to `$HOME`.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct Cwd(pub PathBuf);

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;
    use bevy::prelude::With;

    #[test]
    fn markers_are_components_and_query_filterable() {
        let mut world = World::new();
        let workspace = world.spawn(WorkspaceMarker).id();
        let pane = world.spawn(PaneMarker).id();
        let surface = world.spawn(SurfaceMarker).id();

        let mut q = world.query_filtered::<Entity, With<WorkspaceMarker>>();
        let workspaces: Vec<_> = q.iter(&world).collect();
        assert_eq!(workspaces, vec![workspace]);

        let mut q = world.query_filtered::<Entity, With<PaneMarker>>();
        let panes: Vec<_> = q.iter(&world).collect();
        assert_eq!(panes, vec![pane]);

        let mut q = world.query_filtered::<Entity, With<SurfaceMarker>>();
        let surfaces: Vec<_> = q.iter(&world).collect();
        assert_eq!(surfaces, vec![surface]);
    }

    #[test]
    fn copy_mode_default_is_off() {
        assert_eq!(CopyMode::default(), CopyMode::Off);
    }

    #[test]
    fn cwd_holds_a_pathbuf() {
        let c = Cwd(PathBuf::from("/tmp/x"));
        assert_eq!(c.0, PathBuf::from("/tmp/x"));
        assert_eq!(c.clone().0, PathBuf::from("/tmp/x"));
    }
}
