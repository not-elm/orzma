//! The `Mux` aggregate: owns every slotmap and the active pointers, and
//! exposes the mutation API (each op returns `Vec<MuxEvent>`) plus queries.

use crate::direction::{CycleDirection, PaneDirection, SwapOffset};
use crate::error::{MuxError, MuxResult};
use crate::event::{MuxEvent, SurfaceEntry};
use crate::geometry::{Rect, split_cells};
use crate::id::{NodeId, PaneId, SessionId, SplitId, SurfaceId, WorkspaceId};
use crate::snapshot::{PaneSnapshot, SessionSnapshot, SurfaceState, WorkspaceSnapshot};
use crate::surface::{Surface, SurfaceKind};
use crate::tree::{LayoutNode, Pane, Side, Split, SplitOrientation};
use slotmap::{Key, SlotMap};
use std::path::PathBuf;

/// Hard floor on a leaf pane's cell count along the left-right axis.
const MIN_PANE_COLS: u16 = 10;

/// Hard floor on a leaf pane's cell count along the top-bottom axis.
const MIN_PANE_ROWS: u16 = 3;

struct Session {
    workspaces: Vec<WorkspaceId>,
    active: WorkspaceId,
}

struct Workspace {
    root: NodeId,
    active_pane: PaneId,
    name: String,
    #[expect(
        dead_code,
        reason = "used as a creation-order key; queried by future workspace-list API"
    )]
    created_at: u32,
    size: Option<(u16, u16)>,
}

/// The multiplexer aggregate root.
pub struct MultiPlexer {
    sessions: SlotMap<SessionId, Session>,
    active_session: SessionId,
    workspaces: SlotMap<WorkspaceId, Workspace>,
    splits: SlotMap<SplitId, Split>,
    panes: SlotMap<PaneId, Pane>,
    surfaces: SlotMap<SurfaceId, Surface>,
    name_counter: u32,
}

impl Default for MultiPlexer {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiPlexer {
    /// Builds the initial state: one default session with one workspace
    /// holding a single terminal pane. Active pointers are valid
    /// immediately. (Initial state is conveyed as a snapshot, so no events.)
    pub fn new() -> Self {
        let mut mux = MultiPlexer {
            sessions: SlotMap::with_key(),
            active_session: SessionId::default(),
            workspaces: SlotMap::with_key(),
            splits: SlotMap::with_key(),
            panes: SlotMap::with_key(),
            surfaces: SlotMap::with_key(),
            name_counter: 0,
        };
        let surface = mux.surfaces.insert(Surface {
            kind: SurfaceKind::Terminal,
            cwd: None,
        });
        let pane = mux.panes.insert(Pane {
            surfaces: vec![surface],
            active_surface: surface,
            parent: None,
        });
        let created_at = mux.name_counter;
        mux.name_counter += 1;
        let workspace = mux.workspaces.insert(Workspace {
            root: NodeId::Pane(pane),
            active_pane: pane,
            name: format!("{created_at}"),
            created_at,
            size: None,
        });
        let session = mux.sessions.insert(Session {
            workspaces: vec![workspace],
            active: workspace,
        });
        mux.active_session = session;
        mux
    }

    /// The active session's id.
    pub fn active_session(&self) -> SessionId {
        self.active_session
    }

    /// The active session's active workspace.
    pub fn active_workspace(&self) -> WorkspaceId {
        self.sessions[self.active_session].active
    }

    /// The workspace's focused pane.
    pub fn active_pane(&self, workspace: WorkspaceId) -> MuxResult<PaneId> {
        Ok(self.workspace(workspace)?.active_pane)
    }

    /// The pane's focused surface.
    pub fn active_surface(&self, pane: PaneId) -> MuxResult<SurfaceId> {
        Ok(self.pane(pane)?.active_surface)
    }

    /// A pane's surfaces in creation order.
    pub fn surfaces(&self, pane: PaneId) -> MuxResult<Vec<SurfaceId>> {
        Ok(self.pane(pane)?.surfaces.clone())
    }

    /// A surface's kind.
    pub fn surface_kind(&self, surface: SurfaceId) -> MuxResult<SurfaceKind> {
        Ok(self.surface(surface)?.kind.clone())
    }

    /// Each pane's normalized rect, DFS first-child-first. Port of `pane_bounds`.
    pub fn pane_bounds(&self, workspace: WorkspaceId) -> MuxResult<Vec<(PaneId, Rect)>> {
        let root = self.workspace(workspace)?.root;
        let mut out = Vec::new();
        self.walk_bounds(
            root,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            &mut out,
        );
        Ok(out)
    }

    /// Resolve each pane's integer `cols × rows` for a workspace of total
    /// `cols × rows`, distributing cells down the tree so children sum exactly
    /// to the parent. Port of the `split_cells`-based recursion.
    pub fn resolve_sizes(
        &self,
        workspace: WorkspaceId,
        cols: u16,
        rows: u16,
    ) -> MuxResult<Vec<(PaneId, (u16, u16))>> {
        let root = self.workspace(workspace)?.root;
        let mut out = Vec::new();
        self.resolve_node(root, cols, rows, &mut out);
        Ok(out)
    }

    /// Every pane in a workspace, DFS first-child-first (matches the old
    /// `ordered_panes`): leftmost leaf first.
    pub fn ordered_panes(&self, workspace: WorkspaceId) -> MuxResult<Vec<PaneId>> {
        let root = self.workspace(workspace)?.root;
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node {
                NodeId::Pane(p) => out.push(p),
                NodeId::Split(s) => {
                    let split = &self.splits[s];
                    // NOTE: push second THEN first so first pops (and is visited) first,
                    // preserving DFS leftmost-leaf-first order that layout.rs relies on.
                    stack.push(split.second);
                    stack.push(split.first);
                }
            }
        }
        Ok(out)
    }

    /// Split `pane` along `orientation`, inserting a new pane (seeded with one
    /// `surface_kind` surface) on `side`. Reparents `pane` and the new pane
    /// under a fresh `Split` (ratio `0.5`) that takes `pane`'s old layout slot.
    /// The new pane becomes the workspace's active pane.
    ///
    /// Returns `[PaneCreated, LayoutChanged{root: Pane(pane)}, ActivePaneChanged,
    /// PaneResized*]`.
    pub fn split_pane(
        &mut self,
        pane: PaneId,
        orientation: SplitOrientation,
        side: Side,
        surface_kind: SurfaceKind,
        cwd: Option<PathBuf>,
    ) -> MuxResult<Vec<MuxEvent>> {
        let workspace = self.owning_workspace_of_pane(pane)?;
        let before = self.resolved_sizes_or_empty(workspace);
        let old_parent = self.pane(pane)?.parent;

        let surface = self.surfaces.insert(Surface {
            kind: surface_kind.clone(),
            cwd,
        });
        let new_pane = self.panes.insert(Pane {
            surfaces: vec![surface],
            active_surface: surface,
            parent: None,
        });

        let (first, second) = match side {
            Side::Before => (NodeId::Pane(new_pane), NodeId::Pane(pane)),
            Side::After => (NodeId::Pane(pane), NodeId::Pane(new_pane)),
        };
        let split = self
            .splits
            .insert(Split::new(orientation, 0.5, first, second, old_parent));
        let split_node = NodeId::Split(split);

        self.panes[pane].parent = Some(split_node);
        self.panes[new_pane].parent = Some(split_node);
        self.rewrite_child_pointer(workspace, old_parent, NodeId::Pane(pane), split_node);

        self.workspaces[workspace].active_pane = new_pane;

        let (cols, rows) = self.node_cell_extent(split_node);
        let subtree = self.build_layout_node(split_node, cols, rows);
        let mut events = vec![
            MuxEvent::PaneCreated {
                pane: new_pane,
                workspace,
                surfaces: self.pane_surface_entries(new_pane),
                active_surface: self.panes[new_pane].active_surface,
            },
            // NOTE: `root` is the *replaced* node (the old pane slot), NOT the
            // new split — the consumer uses it as a find-and-replace key to
            // swap that child for `subtree`. Emitting Split(split) would be wrong.
            MuxEvent::LayoutChanged {
                workspace,
                root: NodeId::Pane(pane),
                subtree,
            },
            MuxEvent::ActivePaneChanged {
                workspace,
                pane: new_pane,
            },
        ];
        let after = self.resolved_sizes_or_empty(workspace);
        events.extend(pane_resize_events(&before, &after));
        Ok(events)
    }

    /// Close `pane`: promote its sibling into the grandparent slot, despawn the
    /// pane, its parent split, and its surfaces. Errors if `pane` is the
    /// workspace's only pane.
    ///
    /// Emits `[PaneClosed, SurfaceClosed*, (LayoutChanged | WorkspaceRootChanged),
    /// ActivePaneChanged?, PaneResized*]`.
    pub fn close_pane(&mut self, pane: PaneId) -> MuxResult<Vec<MuxEvent>> {
        let workspace = self.owning_workspace_of_pane(pane)?;
        let before = self.resolved_sizes_or_empty(workspace);
        let parent = self.pane(pane)?.parent;
        let split_id = match parent {
            Some(NodeId::Split(s)) => s,
            _ => return Err(MuxError::CannotCloseLastPaneInWorkspace(workspace)),
        };

        let split = self.splits[split_id].clone();
        let sibling = if split.first == NodeId::Pane(pane) {
            split.second
        } else {
            split.first
        };
        let grandparent = split.parent;

        self.set_node_parent(sibling, grandparent);
        self.rewrite_child_pointer(workspace, grandparent, NodeId::Split(split_id), sibling);

        self.splits.remove(split_id);
        let mut events = self.destroy_pane(pane);

        let reached_root = grandparent.is_none();
        if reached_root {
            let (cols, rows) = self.node_cell_extent(sibling);
            let root = self.build_layout_node(sibling, cols, rows);
            events.push(MuxEvent::WorkspaceRootChanged { workspace, root });
        } else {
            let (cols, rows) = self.node_cell_extent(sibling);
            let subtree = self.build_layout_node(sibling, cols, rows);
            // NOTE: `root` is the now-removed split id used as a find-and-replace
            // key (the consumer swaps that subtree for the surviving sibling); it
            // is intentionally a stale id, not a live arena reference.
            events.push(MuxEvent::LayoutChanged {
                workspace,
                root: NodeId::Split(split_id),
                subtree,
            });
        }

        if self.workspaces[workspace].active_pane == pane {
            let survivor = self.leftmost_pane(sibling);
            self.workspaces[workspace].active_pane = survivor;
            events.push(MuxEvent::ActivePaneChanged {
                workspace,
                pane: survivor,
            });
        }

        let after = self.resolved_sizes_or_empty(workspace);
        events.extend(pane_resize_events(&before, &after));
        Ok(events)
    }

    /// Focus `pane`: set its workspace's active pane. Emits
    /// `[ActivePaneChanged]`, or `[]` if already active.
    pub fn focus_pane(&mut self, pane: PaneId) -> MuxResult<Vec<MuxEvent>> {
        let workspace = self.owning_workspace_of_pane(pane)?;
        if self.workspaces[workspace].active_pane == pane {
            return Ok(vec![]);
        }
        self.workspaces[workspace].active_pane = pane;
        Ok(vec![MuxEvent::ActivePaneChanged { workspace, pane }])
    }

    /// Move focus to the next/previous pane in DFS order (wrapping). Emits
    /// `[ActivePaneChanged]`, or `[]` if it resolves to the current pane.
    pub fn cycle_pane(
        &mut self,
        workspace: WorkspaceId,
        direction: CycleDirection,
    ) -> MuxResult<Vec<MuxEvent>> {
        let ordered = self.ordered_panes(workspace)?;
        if ordered.is_empty() {
            return Ok(vec![]);
        }
        let active = self.workspaces[workspace].active_pane;
        let i = ordered.iter().position(|p| *p == active).unwrap_or(0);
        let len = ordered.len() as isize;
        let delta = match direction {
            CycleDirection::Prev => -1,
            CycleDirection::Next => 1,
        };
        let j = ((i as isize + delta).rem_euclid(len)) as usize;
        let target = ordered[j];
        self.focus_pane(target)
    }

    /// Move focus to the geometric neighbor of `pane` in `direction` (with
    /// wrap-around). Emits `[ActivePaneChanged]`, or `[]` when no neighbor
    /// exists or it resolves to `pane`.
    pub fn navigate(&mut self, pane: PaneId, direction: PaneDirection) -> MuxResult<Vec<MuxEvent>> {
        let workspace = self.owning_workspace_of_pane(pane)?;
        let bounds = self.pane_bounds(workspace)?;
        match crate::direction::pane_in_direction(&bounds, pane, direction, |_| 0) {
            Some(target) => self.focus_pane(target),
            None => Ok(vec![]),
        }
    }

    /// Set the pane's focused surface. Emits `[ActiveSurfaceChanged]`, or `[]`
    /// if unchanged. Errors if `surface` is not one of the pane's surfaces.
    pub fn set_active_surface(
        &mut self,
        pane: PaneId,
        surface: SurfaceId,
    ) -> MuxResult<Vec<MuxEvent>> {
        let p = self.pane(pane)?;
        if !p.surfaces.contains(&surface) {
            return Err(MuxError::SurfaceNotFound(surface));
        }
        if p.active_surface == surface {
            return Ok(vec![]);
        }
        self.panes[pane].active_surface = surface;
        Ok(vec![MuxEvent::ActiveSurfaceChanged { pane, surface }])
    }

    /// Sets the active surface, resolving its pane internally.
    ///
    /// Wire callers that hold only a `SurfaceId` need not know the owning pane.
    /// Emits `[ActiveSurfaceChanged]`, or `[]` if already active.
    /// Errors `SurfaceNotFound` if unknown.
    pub fn set_active_surface_by_surface(
        &mut self,
        surface: SurfaceId,
    ) -> MuxResult<Vec<MuxEvent>> {
        let pane = self.pane_of_surface(surface)?;
        self.set_active_surface(pane, surface)
    }

    /// Swap `pane` with its prev/next neighbor in DFS leaf order, keeping each
    /// SLOT's ratio (slot-pinned, since ratios live on the split slots).
    /// `[]` (no-op) for a single-pane workspace.
    ///
    /// Emits a `LayoutChanged` for each affected slot, followed by `PaneResized*`.
    pub fn swap_pane(&mut self, pane: PaneId, offset: SwapOffset) -> MuxResult<Vec<MuxEvent>> {
        let workspace = self.owning_workspace_of_pane(pane)?;
        let ordered = self.ordered_panes(workspace)?;
        if ordered.len() < 2 {
            return Ok(vec![]);
        }
        let i = ordered
            .iter()
            .position(|p| *p == pane)
            .ok_or(MuxError::PaneNotFound(pane))?;
        let len = ordered.len() as isize;
        let delta = match offset {
            SwapOffset::Prev => -1,
            SwapOffset::Next => 1,
        };
        let j = ((i as isize + delta).rem_euclid(len)) as usize;
        let other = ordered[j];
        if other == pane {
            return Ok(vec![]);
        }

        let before = self.resolved_sizes_or_empty(workspace);
        let pa = self.pane(pane)?.parent.ok_or(MuxError::MissingParentCell)?;
        let pb = self
            .pane(other)?
            .parent
            .ok_or(MuxError::MissingParentCell)?;
        let side_a = self.slot_side_of(pa, NodeId::Pane(pane));
        let side_b = self.slot_side_of(pb, NodeId::Pane(other));

        self.write_split_slot(pa, side_a, NodeId::Pane(other));
        self.write_split_slot(pb, side_b, NodeId::Pane(pane));
        self.set_node_parent(NodeId::Pane(pane), Some(pb));
        self.set_node_parent(NodeId::Pane(other), Some(pa));

        let mut events = Vec::new();
        let mut affected = vec![pa];
        if pb != pa {
            affected.push(pb);
        }
        for node in affected {
            let (cols, rows) = self.node_cell_extent(node);
            let subtree = self.build_layout_node(node, cols, rows);
            events.push(MuxEvent::LayoutChanged {
                workspace,
                root: node,
                subtree,
            });
        }
        let after = self.resolved_sizes_or_empty(workspace);
        events.extend(pane_resize_events(&before, &after));
        Ok(events)
    }

    /// Store the workspace's terminal size. Emits `WorkspaceResized` followed by
    /// a `PaneResized` for every pane whose resolved cell size changed.
    pub fn set_workspace_size(
        &mut self,
        workspace: WorkspaceId,
        cols: u16,
        rows: u16,
    ) -> MuxResult<Vec<MuxEvent>> {
        let before = self.resolved_sizes_or_empty(workspace);
        let prev_size = self.workspace(workspace)?.size;
        self.workspaces[workspace].size = Some((cols, rows));
        let after = self.resolve_sizes(workspace, cols, rows)?;

        let mut events = Vec::new();
        if prev_size != Some((cols, rows)) {
            events.push(MuxEvent::WorkspaceResized {
                workspace,
                cols,
                rows,
            });
        }
        events.extend(pane_resize_events(&before, &after));
        Ok(events)
    }

    /// Resize the split controlling `pane`'s extent in `direction` by up to
    /// `amount` cells, clamped by descendant min-cell floors. `[]` (no-op)
    /// when the workspace has no size or no matching ancestor split.
    ///
    /// Emits `[LayoutRatioChanged, PaneResized*]`.
    pub fn resize_pane(
        &mut self,
        pane: PaneId,
        direction: PaneDirection,
        amount: u16,
    ) -> MuxResult<Vec<MuxEvent>> {
        let workspace = self.owning_workspace_of_pane(pane)?;
        let Some((ws_cols, ws_rows)) = self.workspace(workspace)?.size else {
            return Ok(vec![]);
        };
        if ws_cols == 0 || ws_rows == 0 {
            return Ok(vec![]);
        }

        let (axis, sign) = direction_to_axis_sign(direction);
        let Some(ancestor) = self.find_matching_ancestor(NodeId::Pane(pane), axis) else {
            return Ok(vec![]);
        };

        let workspace_p = match axis {
            SplitOrientation::Horizontal => ws_cols,
            SplitOrientation::Vertical => ws_rows,
        };
        let min_cells = match axis {
            SplitOrientation::Horizontal => MIN_PANE_COLS,
            SplitOrientation::Vertical => MIN_PANE_ROWS,
        };
        let p_ancestor = self.compute_p_at(ancestor, axis, workspace_p);

        let split = &self.splits[ancestor];
        let (lhs, rhs) = (split.first, split.second);
        let (current_lhs, current_rhs) = split_cells(p_ancestor, split.ratio());

        let (shrink_cell, shrink_p) = if sign > 0 {
            (rhs, current_rhs)
        } else {
            (lhs, current_lhs)
        };

        let applied = self.available_to_shrink(shrink_cell, axis, shrink_p, min_cells, amount);
        if applied == 0 {
            return Ok(vec![]);
        }

        let signed_delta = sign * applied as i16;
        let new_lhs_cells: u16 = (i32::from(current_lhs) + i32::from(signed_delta))
            .clamp(0, i32::from(p_ancestor)) as u16;
        let ratio = f32::from(new_lhs_cells) / f32::from(p_ancestor);

        let before = self.resolve_sizes(workspace, ws_cols, ws_rows)?;
        self.splits[ancestor].set_ratio(ratio);
        let after = self.resolve_sizes(workspace, ws_cols, ws_rows)?;

        let mut events = vec![MuxEvent::LayoutRatioChanged {
            split: ancestor,
            ratio: self.splits[ancestor].ratio(),
        }];
        events.extend(pane_resize_events(&before, &after));
        Ok(events)
    }

    /// Creates a workspace in the active session, seeding one terminal pane and
    /// surface. Makes the new workspace active. `name` overrides the default
    /// monotonic name when `Some`, applied atomically at creation.
    ///
    /// Returns `[WorkspaceCreated, PaneCreated, WorkspaceSelected, ActivePaneChanged]`.
    pub fn new_workspace(&mut self, name: Option<String>) -> MuxResult<Vec<MuxEvent>> {
        let session = self.active_session;
        let surface = self.surfaces.insert(Surface {
            kind: SurfaceKind::Terminal,
            cwd: None,
        });
        let pane = self.panes.insert(Pane {
            surfaces: vec![surface],
            active_surface: surface,
            parent: None,
        });
        let created_at = self.name_counter;
        self.name_counter += 1;
        let name = name.unwrap_or_else(|| format!("{created_at}"));
        let workspace = self.workspaces.insert(Workspace {
            root: NodeId::Pane(pane),
            active_pane: pane,
            name: name.clone(),
            created_at,
            size: None,
        });
        self.sessions[session].workspaces.push(workspace);
        self.sessions[session].active = workspace;
        Ok(vec![
            MuxEvent::WorkspaceCreated {
                session,
                workspace,
                name,
            },
            MuxEvent::PaneCreated {
                pane,
                workspace,
                surfaces: self.pane_surface_entries(pane),
                active_surface: self.panes[pane].active_surface,
            },
            MuxEvent::WorkspaceSelected { session, workspace },
            MuxEvent::ActivePaneChanged { workspace, pane },
        ])
    }

    /// Selects an existing workspace as the active session's active workspace.
    ///
    /// Returns `[WorkspaceSelected]`, or `[]` if already active. Errors
    /// `WorkspaceNotFound` for unknown ids.
    pub fn select_workspace(&mut self, workspace: WorkspaceId) -> MuxResult<Vec<MuxEvent>> {
        self.workspace(workspace)?;
        let session = self.active_session;
        if self.sessions[session].active == workspace {
            return Ok(vec![]);
        }
        self.sessions[session].active = workspace;
        Ok(vec![MuxEvent::WorkspaceSelected { session, workspace }])
    }

    /// Renames a workspace. Emits `WorkspaceRenamed` only when the name
    /// actually changes (port of `commands::rename_workspace` changed-only).
    pub fn rename_workspace(
        &mut self,
        workspace: WorkspaceId,
        name: String,
    ) -> MuxResult<Vec<MuxEvent>> {
        self.workspace(workspace)?;
        if self.workspaces[workspace].name == name {
            return Ok(vec![]);
        }
        self.workspaces[workspace].name = name.clone();
        Ok(vec![MuxEvent::WorkspaceRenamed { workspace, name }])
    }

    /// Destroys a workspace and all its panes and surfaces (cascade). If the
    /// destroyed workspace was the session's active workspace, the session
    /// re-points to another workspace (the previous one in the vec) if one
    /// exists; otherwise a fresh workspace is auto-created, keeping the session
    /// always valid.
    ///
    /// Emits `PaneClosed` per pane, `SurfaceClosed` per surface of that pane,
    /// then `WorkspaceDestroyed`. If the active pointer had to change a
    /// `WorkspaceSelected` is appended. If a replacement was auto-created, a
    /// full `new_workspace` event sequence is prepended.
    pub fn close_workspace(&mut self, workspace: WorkspaceId) -> MuxResult<Vec<MuxEvent>> {
        let root = self.workspace(workspace)?.root;
        let session = self.active_session;
        let was_active = self.sessions[session].active == workspace;

        let mut events = self.cascade_destroy_subtree(root);
        self.workspaces.remove(workspace);
        self.sessions[session].workspaces.retain(|w| *w != workspace);
        events.push(MuxEvent::WorkspaceDestroyed { workspace });

        if !was_active {
            return Ok(events);
        }

        // The active workspace was destroyed: re-point the session to the
        // previous workspace, or auto-create a replacement so the session is
        // never left empty (the always-one-workspace invariant).
        if let Some(&remaining) = self.sessions[session].workspaces.last() {
            self.sessions[session].active = remaining;
            events.push(MuxEvent::WorkspaceSelected {
                session,
                workspace: remaining,
            });
            Ok(events)
        } else {
            let mut replacement = self.new_workspace(None)?;
            replacement.append(&mut events);
            Ok(replacement)
        }
    }

    /// Adds a surface to a pane without changing the active surface.
    ///
    /// Returns `[SurfaceSpawned]`.
    pub fn spawn_surface(
        &mut self,
        pane: PaneId,
        kind: SurfaceKind,
        cwd: Option<PathBuf>,
    ) -> MuxResult<Vec<MuxEvent>> {
        self.pane(pane)?;
        let surface = self.surfaces.insert(Surface {
            kind: kind.clone(),
            cwd: cwd.clone(),
        });
        self.panes[pane].surfaces.push(surface);
        Ok(vec![MuxEvent::SurfaceSpawned {
            pane,
            surface,
            kind,
            cwd: cwd.unwrap_or_default(),
        }])
    }

    /// Moves a surface into a new pane created by splitting its current pane.
    ///
    /// Emits `[PaneCreated, LayoutChanged, ActivePaneChanged, ActiveSurfaceChanged?,
    /// SurfaceMoved, PaneResized*]`.
    ///
    /// Errors `CannotRemoveLastSurface` if the source pane has only one surface.
    pub fn break_surface_to_pane(
        &mut self,
        surface: SurfaceId,
        orientation: SplitOrientation,
        side: Side,
    ) -> MuxResult<Vec<MuxEvent>> {
        let source_pane = self.pane_of_surface(surface)?;
        if self.panes[source_pane].surfaces.len() < 2 {
            return Err(MuxError::CannotRemoveLastSurface(source_pane));
        }
        let workspace = self.owning_workspace_of_pane(source_pane)?;
        let before = self.resolved_sizes_or_empty(workspace);

        let mut events = self.split_pane_empty(source_pane, orientation, side)?;

        let new_pane = match events[0] {
            MuxEvent::PaneCreated { pane, .. } => pane,
            _ => unreachable!("split_pane_empty first event must be PaneCreated"),
        };

        let old_bootstrap = self.panes[new_pane].surfaces[0];
        self.surfaces.remove(old_bootstrap);
        self.panes[new_pane].surfaces.clear();
        self.panes[new_pane].surfaces.push(surface);
        self.panes[new_pane].active_surface = surface;

        // NOTE: split_pane_empty stamps PaneCreated with the bootstrap surface
        // (Terminal kind, empty cwd); correct the manifest to the MOVED surface
        // so subscribers initialize the right surface type and id.
        if let MuxEvent::PaneCreated {
            surfaces,
            active_surface,
            ..
        } = &mut events[0]
        {
            *surfaces = self.pane_surface_entries(new_pane);
            *active_surface = self.panes[new_pane].active_surface;
        }

        // NOTE: split_pane_empty builds LayoutChanged while new_pane still holds
        // the bootstrap Terminal surface; the moved surface may have a different
        // kind. Patch the new_pane leaf in the LayoutChanged subtree to match the
        // corrected PaneCreated surface kind so the wire is self-consistent.
        let moved_kind = self.surfaces[surface].kind.clone();
        if let MuxEvent::LayoutChanged { subtree, .. } = &mut events[1] {
            patch_pane_kind_in_layout(subtree, new_pane, &moved_kind);
        }

        self.panes[source_pane].surfaces.retain(|s| *s != surface);
        let src_active = self.panes[source_pane].active_surface;
        if src_active == surface {
            let new_active = self.panes[source_pane].surfaces[0];
            self.panes[source_pane].active_surface = new_active;
            events.push(MuxEvent::ActiveSurfaceChanged {
                pane: source_pane,
                surface: new_active,
            });
        }

        // NOTE: PaneCreated (already in events[0]) adds `surface` to `new_pane`'s
        // manifest; SurfaceMoved tells consumers to remove it from `source_pane`.
        // Emit after PaneCreated so apply-order never double-counts the surface.
        events.push(MuxEvent::SurfaceMoved {
            surface,
            from_pane: source_pane,
            to_pane: new_pane,
        });

        let after = self.resolved_sizes_or_empty(workspace);
        events.extend(pane_resize_events(&before, &after));
        Ok(events)
    }

    /// Removes a surface from its pane (re-points `active_surface` if needed).
    ///
    /// Errors `SurfaceNotFound` if unknown, or `CannotRemoveLastSurface` if the
    /// pane has only one surface.
    pub fn close_surface(&mut self, surface: SurfaceId) -> MuxResult<Vec<MuxEvent>> {
        let pane = self.pane_of_surface(surface)?;
        if self.panes[pane].surfaces.len() < 2 {
            return Err(MuxError::CannotRemoveLastSurface(pane));
        }
        self.panes[pane].surfaces.retain(|s| *s != surface);
        let was_active = self.panes[pane].active_surface == surface;
        self.surfaces.remove(surface);
        let mut events = vec![MuxEvent::SurfaceClosed { surface }];
        if was_active {
            let new_active = self.panes[pane].surfaces[0];
            self.panes[pane].active_surface = new_active;
            events.push(MuxEvent::ActiveSurfaceChanged {
                pane,
                surface: new_active,
            });
        }
        Ok(events)
    }

    /// The workspace's stored terminal size, or `None` if not yet set.
    pub fn workspace_size(&self, workspace: WorkspaceId) -> MuxResult<Option<(u16, u16)>> {
        Ok(self.workspace(workspace)?.size)
    }

    /// The workspace's display name.
    pub fn workspace_name(&self, workspace: WorkspaceId) -> MuxResult<&str> {
        Ok(&self.workspace(workspace)?.name)
    }

    /// The root `NodeId` of the workspace's layout tree.
    pub fn workspace_root(&self, workspace: WorkspaceId) -> MuxResult<NodeId> {
        Ok(self.workspace(workspace)?.root)
    }

    /// Serializes a workspace's tree to the wire `LayoutNode`, resolving each
    /// pane's cell size from the workspace's logical size (0 when unset).
    pub fn workspace_layout(&self, workspace: WorkspaceId) -> MuxResult<LayoutNode> {
        let ws = self.workspace(workspace)?;
        let (cols, rows) = ws.size.unwrap_or((0, 0));
        Ok(self.build_layout_node(ws.root, cols, rows))
    }

    /// Sets a surface's working directory. Emits `SurfaceCwdChanged` only when
    /// the cwd actually changes; an empty path is ignored, since the empty
    /// `PathBuf` is the wire "no cwd" sentinel and must never enter the event
    /// stream as a real cwd (consumers treat an empty cwd as "absent").
    pub fn set_surface_cwd(
        &mut self,
        surface: SurfaceId,
        cwd: PathBuf,
    ) -> MuxResult<Vec<MuxEvent>> {
        self.surface(surface)?;
        if cwd.as_os_str().is_empty() || self.surfaces[surface].cwd.as_ref() == Some(&cwd) {
            return Ok(vec![]);
        }
        self.surfaces[surface].cwd = Some(cwd.clone());
        Ok(vec![MuxEvent::SurfaceCwdChanged { surface, cwd }])
    }

    /// Returns all session ids sorted by insertion order.
    ///
    /// Order is stable only because no public API removes sessions; if session
    /// removal is ever added, callers must not rely on index-based ordering.
    pub fn sessions(&self) -> Vec<SessionId> {
        // NOTE: SlotMap iteration order is not guaranteed to match insertion
        // order. The active session is the only session created by `new()`, and
        // subsequent sessions are not yet created by any public API. To ensure
        // deterministic ordering across all future callers, we collect and sort
        // by each key's raw index bits — the lowest-index slot was inserted
        // first when no removals have occurred (the current invariant).
        let mut ids: Vec<SessionId> = self.sessions.keys().collect();
        ids.sort_by_key(|id| id.data().as_ffi());
        ids
    }

    /// Returns `session`'s workspace ids in creation order.
    pub fn workspaces(&self, session: SessionId) -> MuxResult<Vec<WorkspaceId>> {
        Ok(self
            .sessions
            .get(session)
            .ok_or(MuxError::SessionNotFound(session))?
            .workspaces
            .clone())
    }

    /// Snapshots `session`'s full state for cold-attach.
    ///
    /// The returned `SessionSnapshot` can be serialized to initialize a
    /// remote mirror, which then applies subsequent `MuxEvent` deltas.
    pub fn snapshot(&self, session: SessionId) -> MuxResult<SessionSnapshot> {
        let sess = self
            .sessions
            .get(session)
            .ok_or(MuxError::SessionNotFound(session))?;
        let active_workspace = sess.active;
        let workspace_ids = sess.workspaces.clone();

        let mut workspaces = Vec::with_capacity(workspace_ids.len());
        for ws_id in workspace_ids {
            let ws = self.workspace(ws_id)?;
            let name = ws.name.clone();
            let size = ws.size;
            let active_pane = ws.active_pane;
            let layout = self.workspace_layout(ws_id)?;

            let ordered = self.ordered_panes(ws_id)?;
            let mut panes = Vec::with_capacity(ordered.len());
            for pane_id in ordered {
                let pane = &self.panes[pane_id];
                let active_surface = pane.active_surface;
                let surfaces = self
                    .pane_surface_entries(pane_id)
                    .into_iter()
                    .map(|e| SurfaceState {
                        surface: e.surface,
                        kind: e.kind,
                        cwd: e.cwd,
                    })
                    .collect();
                panes.push(PaneSnapshot {
                    pane: pane_id,
                    surfaces,
                    active_surface,
                });
            }

            workspaces.push(WorkspaceSnapshot {
                workspace: ws_id,
                name,
                layout,
                size,
                active_pane,
                panes,
            });
        }

        Ok(SessionSnapshot {
            session,
            active_workspace,
            workspaces,
        })
    }

    /// Returns `pane`'s resolved cell size, or `None` when the owning workspace
    /// has no size yet (startup / zero clients). Used to seed a freshly spawned
    /// surface's PTY/Vt at the right size instead of a fixed 80x24.
    pub fn resolved_pane_size(&self, pane: PaneId) -> Option<(u16, u16)> {
        let (cols, rows) = self.node_cell_extent(NodeId::Pane(pane));
        (cols > 0 && rows > 0).then_some((cols, rows))
    }

    fn walk_bounds(&self, node: NodeId, bounds: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match node {
            NodeId::Pane(p) => out.push((p, bounds)),
            NodeId::Split(s) => {
                let split = &self.splits[s];
                let r = split.ratio();
                match split.orientation {
                    SplitOrientation::Horizontal => {
                        let lw = bounds.w * r;
                        self.walk_bounds(split.first, Rect { w: lw, ..bounds }, out);
                        self.walk_bounds(
                            split.second,
                            Rect {
                                x: bounds.x + lw,
                                w: bounds.w - lw,
                                ..bounds
                            },
                            out,
                        );
                    }
                    SplitOrientation::Vertical => {
                        let lh = bounds.h * r;
                        self.walk_bounds(split.first, Rect { h: lh, ..bounds }, out);
                        self.walk_bounds(
                            split.second,
                            Rect {
                                y: bounds.y + lh,
                                h: bounds.h - lh,
                                ..bounds
                            },
                            out,
                        );
                    }
                }
            }
        }
    }

    fn resolve_node(
        &self,
        node: NodeId,
        cols: u16,
        rows: u16,
        out: &mut Vec<(PaneId, (u16, u16))>,
    ) {
        match node {
            NodeId::Pane(p) => out.push((p, (cols, rows))),
            NodeId::Split(s) => {
                let split = &self.splits[s];
                match split.orientation {
                    SplitOrientation::Horizontal => {
                        let (lc, rc) = split_cells(cols, split.ratio());
                        self.resolve_node(split.first, lc, rows, out);
                        self.resolve_node(split.second, rc, rows, out);
                    }
                    SplitOrientation::Vertical => {
                        let (lr, rr) = split_cells(rows, split.ratio());
                        self.resolve_node(split.first, cols, lr, out);
                        self.resolve_node(split.second, cols, rr, out);
                    }
                }
            }
        }
    }

    fn pane_of_surface(&self, surface: SurfaceId) -> MuxResult<PaneId> {
        self.surface(surface)?;
        self.panes
            .iter()
            .find(|(_, p)| p.surfaces.contains(&surface))
            .map(|(id, _)| id)
            .ok_or(MuxError::SurfaceNotFound(surface))
    }

    fn split_pane_empty(
        &mut self,
        pane: PaneId,
        orientation: SplitOrientation,
        side: Side,
    ) -> MuxResult<Vec<MuxEvent>> {
        let workspace = self.owning_workspace_of_pane(pane)?;
        let old_parent = self.pane(pane)?.parent;

        let bootstrap_surface = self.surfaces.insert(Surface {
            kind: SurfaceKind::Terminal,
            cwd: None,
        });
        let new_pane = self.panes.insert(Pane {
            surfaces: vec![bootstrap_surface],
            active_surface: bootstrap_surface,
            parent: None,
        });

        let (first, second) = match side {
            Side::Before => (NodeId::Pane(new_pane), NodeId::Pane(pane)),
            Side::After => (NodeId::Pane(pane), NodeId::Pane(new_pane)),
        };
        let split = self
            .splits
            .insert(Split::new(orientation, 0.5, first, second, old_parent));
        let split_node = NodeId::Split(split);

        self.panes[pane].parent = Some(split_node);
        self.panes[new_pane].parent = Some(split_node);
        self.rewrite_child_pointer(workspace, old_parent, NodeId::Pane(pane), split_node);

        self.workspaces[workspace].active_pane = new_pane;

        let (cols, rows) = self.node_cell_extent(split_node);
        let subtree = self.build_layout_node(split_node, cols, rows);
        Ok(vec![
            MuxEvent::PaneCreated {
                pane: new_pane,
                workspace,
                surfaces: self.pane_surface_entries(new_pane),
                active_surface: self.panes[new_pane].active_surface,
            },
            MuxEvent::LayoutChanged {
                workspace,
                root: NodeId::Pane(pane),
                subtree,
            },
            MuxEvent::ActivePaneChanged {
                workspace,
                pane: new_pane,
            },
        ])
    }

    fn workspace(&self, id: WorkspaceId) -> MuxResult<&Workspace> {
        self.workspaces
            .get(id)
            .ok_or(MuxError::WorkspaceNotFound(id))
    }

    fn pane(&self, id: PaneId) -> MuxResult<&Pane> {
        self.panes.get(id).ok_or(MuxError::PaneNotFound(id))
    }

    fn surface(&self, id: SurfaceId) -> MuxResult<&Surface> {
        self.surfaces.get(id).ok_or(MuxError::SurfaceNotFound(id))
    }

    fn owning_workspace_of_pane(&self, pane: PaneId) -> MuxResult<WorkspaceId> {
        self.pane(pane)?;
        self.owning_workspace_of_node(NodeId::Pane(pane))
            .ok_or(MuxError::PaneNotFound(pane))
    }

    fn owning_workspace_of_node(&self, node: NodeId) -> Option<WorkspaceId> {
        let mut cursor = node;
        loop {
            let parent = match cursor {
                NodeId::Pane(p) => self.panes.get(p)?.parent,
                NodeId::Split(s) => self.splits.get(s)?.parent,
            };
            match parent {
                Some(p) => cursor = p,
                None => break,
            }
        }
        self.workspaces
            .iter()
            .find(|(_, ws)| ws.root == cursor)
            .map(|(id, _)| id)
    }

    fn rewrite_child_pointer(
        &mut self,
        workspace: WorkspaceId,
        parent: Option<NodeId>,
        from: NodeId,
        to: NodeId,
    ) {
        match parent {
            Some(p) => self.replace_split_child(p, from, to),
            None => self.workspaces[workspace].root = to,
        }
    }

    fn replace_split_child(&mut self, parent: NodeId, from: NodeId, to: NodeId) {
        if let NodeId::Split(s) = parent {
            let split = &mut self.splits[s];
            if split.first == from {
                split.first = to;
            } else if split.second == from {
                split.second = to;
            }
        }
    }

    fn slot_side_of(&self, parent: NodeId, child: NodeId) -> Side {
        match parent {
            NodeId::Split(s) if self.splits[s].first == child => Side::Before,
            _ => Side::After,
        }
    }

    fn write_split_slot(&mut self, parent: NodeId, side: Side, child: NodeId) {
        if let NodeId::Split(s) = parent {
            match side {
                Side::Before => self.splits[s].first = child,
                Side::After => self.splits[s].second = child,
            }
        }
    }

    fn set_node_parent(&mut self, node: NodeId, parent: Option<NodeId>) {
        match node {
            NodeId::Pane(p) => self.panes[p].parent = parent,
            NodeId::Split(s) => self.splits[s].parent = parent,
        }
    }

    fn leftmost_pane(&self, start: NodeId) -> PaneId {
        let mut cur = start;
        loop {
            match cur {
                NodeId::Pane(p) => return p,
                NodeId::Split(s) => cur = self.splits[s].first,
            }
        }
    }

    /// Removes `pane` and all of its surfaces from the arena, returning
    /// `[PaneClosed, SurfaceClosed*]`. Does NOT touch the layout tree (splits)
    /// or the owning workspace — callers handle the structural relinking.
    fn destroy_pane(&mut self, pane: PaneId) -> Vec<MuxEvent> {
        let mut events = vec![MuxEvent::PaneClosed { pane }];
        for surface in self.panes[pane].surfaces.clone() {
            events.push(MuxEvent::SurfaceClosed { surface });
            self.surfaces.remove(surface);
        }
        self.panes.remove(pane);
        events
    }

    /// Destroys every split, pane, and surface in the subtree rooted at `root`,
    /// returning `[PaneClosed, SurfaceClosed*]` per pane in DFS (left-first)
    /// order.
    ///
    /// Walking the whole subtree (not just each leaf's parent split) is
    /// required: removing only leaf parents would leak interior splits whose
    /// two children are both splits.
    fn cascade_destroy_subtree(&mut self, root: NodeId) -> Vec<MuxEvent> {
        let mut events = Vec::new();
        let mut split_ids = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node {
                NodeId::Split(s) => {
                    let split = &self.splits[s];
                    stack.push(split.second);
                    stack.push(split.first);
                    split_ids.push(s);
                }
                NodeId::Pane(p) => events.extend(self.destroy_pane(p)),
            }
        }
        for s in split_ids {
            self.splits.remove(s);
        }
        events
    }

    fn node_cell_extent(&self, node: NodeId) -> (u16, u16) {
        let Some(workspace) = self.owning_workspace_of_node(node) else {
            return (0, 0);
        };
        let Some((mut cols, mut rows)) = self.workspaces[workspace].size else {
            return (0, 0);
        };

        let mut path = vec![node];
        let mut cursor = node;
        while let Some(parent) = self.parent_of(cursor) {
            path.push(parent);
            cursor = parent;
        }
        path.reverse();

        for window in path.windows(2) {
            let NodeId::Split(s) = window[0] else {
                continue;
            };
            let split = &self.splits[s];
            let child = window[1];
            match split.orientation {
                SplitOrientation::Horizontal => {
                    let (lc, rc) = split_cells(cols, split.ratio());
                    cols = if child == split.first { lc } else { rc };
                }
                SplitOrientation::Vertical => {
                    let (lr, rr) = split_cells(rows, split.ratio());
                    rows = if child == split.first { lr } else { rr };
                }
            }
        }
        (cols, rows)
    }

    fn parent_of(&self, node: NodeId) -> Option<NodeId> {
        match node {
            NodeId::Pane(p) => self.panes.get(p).and_then(|n| n.parent),
            NodeId::Split(s) => self.splits.get(s).and_then(|n| n.parent),
        }
    }

    fn build_layout_node(&self, node: NodeId, cols: u16, rows: u16) -> LayoutNode {
        match node {
            NodeId::Pane(p) => {
                let pane = &self.panes[p];
                let surface_kind = self.surfaces[pane.active_surface].kind.clone();
                LayoutNode::Pane {
                    id: p,
                    surface_kind,
                    cols,
                    rows,
                }
            }
            NodeId::Split(s) => {
                let split = &self.splits[s];
                let (first, second) = match split.orientation {
                    SplitOrientation::Horizontal => {
                        let (lc, rc) = split_cells(cols, split.ratio());
                        (
                            self.build_layout_node(split.first, lc, rows),
                            self.build_layout_node(split.second, rc, rows),
                        )
                    }
                    SplitOrientation::Vertical => {
                        let (lr, rr) = split_cells(rows, split.ratio());
                        (
                            self.build_layout_node(split.first, cols, lr),
                            self.build_layout_node(split.second, cols, rr),
                        )
                    }
                };
                LayoutNode::Split {
                    id: s,
                    orientation: split.orientation,
                    ratio: split.ratio(),
                    first: Box::new(first),
                    second: Box::new(second),
                }
            }
        }
    }

    fn resolved_sizes_or_empty(&self, workspace: WorkspaceId) -> Vec<(PaneId, (u16, u16))> {
        match self.workspaces.get(workspace).and_then(|w| w.size) {
            Some((cols, rows)) => self
                .resolve_sizes(workspace, cols, rows)
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    fn find_matching_ancestor(&self, start: NodeId, axis: SplitOrientation) -> Option<SplitId> {
        let mut cursor = self.parent_of(start);
        while let Some(node) = cursor {
            if let NodeId::Split(s) = node
                && self.splits[s].orientation == axis
            {
                return Some(s);
            }
            cursor = self.parent_of(node);
        }
        None
    }

    fn compute_p_at(&self, target: SplitId, axis: SplitOrientation, workspace_p: u16) -> u16 {
        let target_node = NodeId::Split(target);
        let mut path = vec![target_node];
        let mut cursor = target_node;
        while let Some(parent) = self.parent_of(cursor) {
            path.push(parent);
            cursor = parent;
        }
        path.reverse();

        let mut p = workspace_p;
        for window in path.windows(2) {
            let NodeId::Split(s) = window[0] else {
                continue;
            };
            let split = &self.splits[s];
            if split.orientation != axis {
                continue;
            }
            let child = window[1];
            let (lc, rc) = split_cells(p, split.ratio());
            p = if child == split.first { lc } else { rc };
        }
        p
    }

    fn satisfies_min_at(
        &self,
        cell: NodeId,
        axis: SplitOrientation,
        p: u16,
        min_cells: u16,
    ) -> bool {
        let s = match cell {
            NodeId::Pane(_) => return p >= min_cells,
            NodeId::Split(s) => s,
        };
        let split = &self.splits[s];
        if split.orientation == axis {
            let (lc, rc) = split_cells(p, split.ratio());
            self.satisfies_min_at(split.first, axis, lc, min_cells)
                && self.satisfies_min_at(split.second, axis, rc, min_cells)
        } else {
            self.satisfies_min_at(split.first, axis, p, min_cells)
                && self.satisfies_min_at(split.second, axis, p, min_cells)
        }
    }

    fn available_to_shrink(
        &self,
        cell: NodeId,
        axis: SplitOrientation,
        p_sub: u16,
        min_cells: u16,
        requested: u16,
    ) -> u16 {
        if p_sub == 0 {
            return 0;
        }
        let upper = requested.min(p_sub);
        let mut max_d = 0u16;
        for d in 1..=upper {
            if self.satisfies_min_at(cell, axis, p_sub - d, min_cells) {
                max_d = d;
            } else {
                break;
            }
        }
        max_d
    }

    fn pane_surface_entries(&self, pane: PaneId) -> Vec<SurfaceEntry> {
        self.panes[pane]
            .surfaces
            .iter()
            .map(|&sid| {
                let s = &self.surfaces[sid];
                SurfaceEntry {
                    surface: sid,
                    kind: s.kind.clone(),
                    cwd: s.cwd.clone().unwrap_or_default(),
                }
            })
            .collect()
    }
}

/// Recursively walk `node` and set `surface_kind` on the `Pane` leaf with the
/// given `id`. Used by `break_surface_to_pane` to patch the `LayoutChanged`
/// subtree produced by `split_pane_empty` (which was built with the bootstrap
/// Terminal surface kind) to reflect the moved surface's actual kind.
fn patch_pane_kind_in_layout(node: &mut LayoutNode, target: PaneId, kind: &SurfaceKind) {
    match node {
        LayoutNode::Pane {
            id, surface_kind, ..
        } if *id == target => {
            *surface_kind = kind.clone();
        }
        LayoutNode::Pane { .. } => {}
        LayoutNode::Split { first, second, .. } => {
            patch_pane_kind_in_layout(first, target, kind);
            patch_pane_kind_in_layout(second, target, kind);
        }
    }
}

fn direction_to_axis_sign(d: PaneDirection) -> (SplitOrientation, i16) {
    match d {
        PaneDirection::Right => (SplitOrientation::Horizontal, 1),
        PaneDirection::Left => (SplitOrientation::Horizontal, -1),
        PaneDirection::Down => (SplitOrientation::Vertical, 1),
        PaneDirection::Up => (SplitOrientation::Vertical, -1),
    }
}

/// Diffs `before` against `after` resolved pane sizes and returns one
/// `MuxEvent::PaneResized` per pane whose size changed (a pane present only in
/// `after` counts as changed). Both slices are `(PaneId, (cols, rows))`. When a
/// workspace has no stored size both slices are empty (see
/// `resolved_sizes_or_empty`), so this naturally emits nothing — never a
/// `PaneResized { cols: 0, rows: 0 }`.
fn pane_resize_events(
    before: &[(PaneId, (u16, u16))],
    after: &[(PaneId, (u16, u16))],
) -> Vec<MuxEvent> {
    let mut events = Vec::new();
    for (pane, (c, r)) in after {
        let changed = before
            .iter()
            .find(|(p, _)| p == pane)
            .map(|(_, prev)| prev != &(*c, *r))
            .unwrap_or(true);
        if changed {
            events.push(MuxEvent::PaneResized {
                pane: *pane,
                cols: *c,
                rows: *r,
            });
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::PaneId;
    use crate::surface::SurfaceKind;

    #[test]
    fn new_seeds_one_session_workspace_pane_surface() {
        let mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let panes = mux.ordered_panes(ws).unwrap();
        assert_eq!(panes.len(), 1);
        let pane = panes[0];
        assert_eq!(mux.surfaces(pane).unwrap().len(), 1);
        assert_eq!(mux.active_pane(ws).unwrap(), pane);
        let surface = mux.active_surface(pane).unwrap();
        assert!(matches!(
            mux.surface_kind(surface).unwrap(),
            SurfaceKind::Terminal
        ));
    }

    #[test]
    fn stale_pane_id_is_pane_not_found() {
        let mux = MultiPlexer::new();
        assert_eq!(
            mux.surfaces(PaneId::default()),
            Err(MuxError::PaneNotFound(PaneId::default()))
        );
    }

    #[test]
    fn single_pane_fills_workspace_and_resolves_full_size() {
        let mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let bounds = mux.pane_bounds(ws).unwrap();
        assert_eq!(
            bounds,
            vec![(
                pane,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                }
            )]
        );
        let sizes = mux.resolve_sizes(ws, 80, 24).unwrap();
        assert_eq!(sizes, vec![(pane, (80, 24))]);
    }

    use Rect;

    fn split_after(mux: &mut MultiPlexer, pane: PaneId, orientation: SplitOrientation) -> PaneId {
        let events = mux
            .split_pane(pane, orientation, Side::After, SurfaceKind::Terminal, None)
            .unwrap();
        match events[0] {
            MuxEvent::PaneCreated { pane, .. } => pane,
            _ => panic!("first event must be PaneCreated"),
        }
    }

    fn root_split(mux: &MultiPlexer, workspace: WorkspaceId) -> SplitId {
        match mux.workspaces[workspace].root {
            NodeId::Split(s) => s,
            NodeId::Pane(_) => panic!("workspace root is a pane, not a split"),
        }
    }

    #[test]
    fn split_pane_inserts_split_reparents_target_and_sets_grows() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let target = mux.active_pane(ws).unwrap();

        let events = mux
            .split_pane(
                target,
                SplitOrientation::Horizontal,
                Side::After,
                SurfaceKind::Terminal,
                None,
            )
            .unwrap();

        let new_pane = match events[0] {
            MuxEvent::PaneCreated {
                pane, workspace, ..
            } => {
                assert_eq!(workspace, ws);
                pane
            }
            _ => panic!("first event must be PaneCreated"),
        };

        let split = root_split(&mux, ws);
        assert_eq!(mux.splits[split].orientation, SplitOrientation::Horizontal);
        assert_eq!(mux.splits[split].ratio(), 0.5);
        assert_eq!(mux.splits[split].first, NodeId::Pane(target));
        assert_eq!(mux.splits[split].second, NodeId::Pane(new_pane));
        assert_eq!(mux.splits[split].parent, None);
        assert_eq!(mux.panes[target].parent, Some(NodeId::Split(split)));
        assert_eq!(mux.panes[new_pane].parent, Some(NodeId::Split(split)));
        assert_eq!(mux.active_pane(ws).unwrap(), new_pane);

        match &events[1] {
            MuxEvent::LayoutChanged {
                workspace,
                root,
                subtree,
            } => {
                assert_eq!(*workspace, ws);
                assert_eq!(*root, NodeId::Pane(target));
                match subtree {
                    LayoutNode::Split { first, second, .. } => {
                        assert!(matches!(**first, LayoutNode::Pane { id, .. } if id == target));
                        assert!(matches!(**second, LayoutNode::Pane { id, .. } if id == new_pane));
                    }
                    _ => panic!("subtree must be a Split"),
                }
            }
            _ => panic!("second event must be LayoutChanged"),
        }

        assert_eq!(
            events[2],
            MuxEvent::ActivePaneChanged {
                workspace: ws,
                pane: new_pane,
            }
        );
    }

    #[test]
    fn split_pane_before_orders_new_then_target() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let target = mux.active_pane(ws).unwrap();

        let events = mux
            .split_pane(
                target,
                SplitOrientation::Vertical,
                Side::Before,
                SurfaceKind::Terminal,
                None,
            )
            .unwrap();
        let new_pane = match events[0] {
            MuxEvent::PaneCreated { pane, .. } => pane,
            _ => panic!("first event must be PaneCreated"),
        };

        let split = root_split(&mux, ws);
        assert_eq!(
            (mux.splits[split].first, mux.splits[split].second),
            (NodeId::Pane(new_pane), NodeId::Pane(target)),
            "Side::Before puts new pane first"
        );
        match &events[1] {
            MuxEvent::LayoutChanged { subtree, .. } => match subtree {
                LayoutNode::Split { first, second, .. } => {
                    assert!(matches!(**first, LayoutNode::Pane { id, .. } if id == new_pane));
                    assert!(matches!(**second, LayoutNode::Pane { id, .. } if id == target));
                }
                _ => panic!("subtree must be a Split"),
            },
            _ => panic!("second event must be LayoutChanged"),
        }
    }

    #[test]
    fn split_in_sized_workspace_emits_pane_resized_for_both_panes() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let original = mux.active_pane(ws).unwrap();
        mux.set_workspace_size(ws, 80, 24).unwrap();

        let events = mux
            .split_pane(
                original,
                SplitOrientation::Horizontal,
                Side::After,
                SurfaceKind::Terminal,
                None,
            )
            .unwrap();

        let new_pane = events
            .iter()
            .find_map(|e| match e {
                MuxEvent::PaneCreated { pane, .. } => Some(*pane),
                _ => None,
            })
            .expect("split emits PaneCreated");

        let resized: Vec<PaneId> = events
            .iter()
            .filter_map(|e| match e {
                MuxEvent::PaneResized { pane, cols, rows } => {
                    assert!(*cols > 0 && *rows > 0, "no zero-size PaneResized");
                    Some(*pane)
                }
                _ => None,
            })
            .collect();
        assert!(resized.contains(&original), "original pane must be resized");
        assert!(resized.contains(&new_pane), "new pane must be resized");
    }

    #[test]
    fn split_in_unsized_workspace_emits_no_pane_resized() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let original = mux.active_pane(ws).unwrap();
        let events = mux
            .split_pane(
                original,
                SplitOrientation::Horizontal,
                Side::After,
                SurfaceKind::Terminal,
                None,
            )
            .unwrap();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, MuxEvent::PaneResized { .. })),
            "an unsized workspace must not emit PaneResized (no 0x0 resize)"
        );
    }

    #[test]
    fn close_pane_promotes_sibling_into_slot_and_despawns_split() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let target = mux.active_pane(ws).unwrap();
        let new_pane = split_after(&mut mux, target, SplitOrientation::Horizontal);
        let split = root_split(&mux, ws);

        let events = mux.close_pane(new_pane).unwrap();

        assert!(mux.panes.get(new_pane).is_none(), "closed pane removed");
        assert!(mux.splits.get(split).is_none(), "parent split removed");
        assert_eq!(mux.workspaces[ws].root, NodeId::Pane(target));
        assert_eq!(mux.panes[target].parent, None);
        assert_eq!(mux.active_pane(ws).unwrap(), target);

        assert_eq!(events[0], MuxEvent::PaneClosed { pane: new_pane });
    }

    #[test]
    fn close_pane_at_root_emits_workspace_root_changed() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let target = mux.active_pane(ws).unwrap();
        let new_pane = split_after(&mut mux, target, SplitOrientation::Horizontal);

        let events = mux.close_pane(new_pane).unwrap();

        assert!(
            events.iter().any(|e| matches!(
                e,
                MuxEvent::WorkspaceRootChanged { workspace, root }
                    if *workspace == ws
                        && matches!(root, LayoutNode::Pane { id, .. } if *id == target)
            )),
            "collapse to root emits WorkspaceRootChanged with the surviving pane: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, MuxEvent::LayoutChanged { .. })),
            "root collapse must not also emit LayoutChanged"
        );
    }

    #[test]
    fn close_non_root_split_emits_layout_changed() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let p1 = mux.active_pane(ws).unwrap();
        let p2 = split_after(&mut mux, p1, SplitOrientation::Horizontal);
        let p3 = split_after(&mut mux, p2, SplitOrientation::Vertical);
        // p3's split (p2's current parent) is the inner SplitV, NOT the root SplitH;
        // closing p3 collapses it without reaching the root.
        let inner_split = match mux.panes[p3].parent {
            Some(NodeId::Split(s)) => s,
            _ => panic!("p3 must have a split parent"),
        };

        let events = mux.close_pane(p3).unwrap();

        assert!(
            events.iter().any(|e| matches!(
                e,
                MuxEvent::LayoutChanged { workspace, root, subtree }
                    if *workspace == ws
                        && matches!(root, NodeId::Split(s) if *s == inner_split)
                        && matches!(subtree, LayoutNode::Pane { id, .. } if *id == p2)
            )),
            "closing a non-root sibling emits LayoutChanged for the removed split's slot: {events:?}"
        );
    }

    #[test]
    fn close_last_pane_errors() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        assert_eq!(
            mux.close_pane(pane),
            Err(MuxError::CannotCloseLastPaneInWorkspace(ws))
        );
    }

    #[test]
    fn closing_pane_removes_its_surfaces() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let target = mux.active_pane(ws).unwrap();
        let new_pane = split_after(&mut mux, target, SplitOrientation::Horizontal);
        let removed_surface = mux.active_surface(new_pane).unwrap();

        let events = mux.close_pane(new_pane).unwrap();

        assert_eq!(
            mux.surface(removed_surface),
            Err(MuxError::SurfaceNotFound(removed_surface)),
            "the closed pane's surface is gone"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                MuxEvent::SurfaceClosed { surface } if *surface == removed_surface
            )),
            "a SurfaceClosed is emitted for each removed surface: {events:?}"
        );
    }

    #[test]
    fn focus_pane_emits_only_on_change() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let p1 = mux.active_pane(ws).unwrap();
        let p2 = split_after(&mut mux, p1, SplitOrientation::Horizontal);
        assert_eq!(mux.active_pane(ws).unwrap(), p2);

        assert_eq!(mux.focus_pane(p2).unwrap(), vec![]);
        assert_eq!(
            mux.focus_pane(p1).unwrap(),
            vec![MuxEvent::ActivePaneChanged {
                workspace: ws,
                pane: p1,
            }]
        );
    }

    #[test]
    fn cycle_pane_wraps_in_dfs_order() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let p1 = mux.active_pane(ws).unwrap();
        let p2 = split_after(&mut mux, p1, SplitOrientation::Horizontal);
        let p3 = split_after(&mut mux, p2, SplitOrientation::Horizontal);
        let ordered = mux.ordered_panes(ws).unwrap();
        assert_eq!(ordered, vec![p1, p2, p3]);

        mux.focus_pane(p1).unwrap();
        assert_eq!(
            mux.cycle_pane(ws, CycleDirection::Next).unwrap(),
            vec![MuxEvent::ActivePaneChanged {
                workspace: ws,
                pane: p2,
            }]
        );
        mux.focus_pane(p1).unwrap();
        assert_eq!(
            mux.cycle_pane(ws, CycleDirection::Prev).unwrap(),
            vec![MuxEvent::ActivePaneChanged {
                workspace: ws,
                pane: p3,
            }],
            "Prev wraps to the last pane"
        );
    }

    #[test]
    fn set_active_surface_changed_only() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let s1 = mux.active_surface(pane).unwrap();
        let s2 = mux.surfaces.insert(Surface {
            kind: SurfaceKind::Terminal,
            cwd: None,
        });
        mux.panes[pane].surfaces.push(s2);

        assert_eq!(mux.set_active_surface(pane, s1).unwrap(), vec![]);
        assert_eq!(
            mux.set_active_surface(pane, s2).unwrap(),
            vec![MuxEvent::ActiveSurfaceChanged { pane, surface: s2 }]
        );
        assert_eq!(
            mux.set_active_surface(pane, SurfaceId::default()),
            Err(MuxError::SurfaceNotFound(SurfaceId::default()))
        );
    }

    fn dir_bounds(mux: &MultiPlexer, ws: WorkspaceId, pane: PaneId) -> Rect {
        mux.pane_bounds(ws)
            .unwrap()
            .into_iter()
            .find(|(p, _)| *p == pane)
            .unwrap()
            .1
    }

    #[test]
    fn horizontal_split_right_then_left_wraps() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let left = mux.active_pane(ws).unwrap();
        let right = split_after(&mut mux, left, SplitOrientation::Horizontal);
        let bounds = mux.pane_bounds(ws).unwrap();

        assert_eq!(
            crate::direction::pane_in_direction(&bounds, left, PaneDirection::Right, |_| 0),
            Some(right)
        );
        assert_eq!(
            crate::direction::pane_in_direction(&bounds, left, PaneDirection::Left, |_| 0),
            Some(right),
            "wrap from left edge picks the rightmost pane"
        );
        assert_eq!(
            crate::direction::pane_in_direction(&bounds, right, PaneDirection::Up, |_| 0),
            None,
            "1xN strip has no candidate on the perpendicular axis"
        );
    }

    #[test]
    fn vertical_split_down_and_up_wrap() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let top = mux.active_pane(ws).unwrap();
        let bottom = split_after(&mut mux, top, SplitOrientation::Vertical);
        let bounds = mux.pane_bounds(ws).unwrap();

        assert_eq!(
            crate::direction::pane_in_direction(&bounds, top, PaneDirection::Down, |_| 0),
            Some(bottom)
        );
        assert_eq!(
            crate::direction::pane_in_direction(&bounds, top, PaneDirection::Up, |_| 0),
            Some(bottom),
            "wrap from top edge"
        );
    }

    #[test]
    fn single_pane_returns_none_in_all_directions() {
        let mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let p = mux.active_pane(ws).unwrap();
        assert_eq!(
            dir_bounds(&mux, ws, p),
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            }
        );
        let bounds = mux.pane_bounds(ws).unwrap();
        for d in [
            PaneDirection::Up,
            PaneDirection::Down,
            PaneDirection::Left,
            PaneDirection::Right,
        ] {
            assert_eq!(
                crate::direction::pane_in_direction(&bounds, p, d, |_| 0),
                None
            );
        }
    }

    #[test]
    fn two_by_two_grid_picks_geometric_neighbor() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let p1 = mux.active_pane(ws).unwrap();
        let p2 = split_after(&mut mux, p1, SplitOrientation::Horizontal);
        let p3 = split_after(&mut mux, p1, SplitOrientation::Vertical);
        let p4 = split_after(&mut mux, p2, SplitOrientation::Vertical);
        let (tl, tr, bl, br) = (p1, p2, p3, p4);

        let r = |e: PaneId| dir_bounds(&mux, ws, e);
        assert!(r(tl).x < r(tr).x && r(tl).y < r(bl).y);
        assert!(r(br).x > r(bl).x && r(br).y > r(tr).y);

        let bounds = mux.pane_bounds(ws).unwrap();
        let nav = |from, d| crate::direction::pane_in_direction(&bounds, from, d, |_| 0);
        assert_eq!(nav(tl, PaneDirection::Right), Some(tr));
        assert_eq!(nav(tl, PaneDirection::Down), Some(bl));
        assert_eq!(nav(br, PaneDirection::Left), Some(bl));
        assert_eq!(nav(br, PaneDirection::Up), Some(tr));
    }

    #[test]
    fn deep_horizontal_split_keeps_immediate_neighbor() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let first = mux.active_pane(ws).unwrap();
        let mut current_pane = first;
        let mut second_last_pane = first;
        for _ in 2..=21_u32 {
            second_last_pane = current_pane;
            current_pane = split_after(&mut mux, current_pane, SplitOrientation::Horizontal);
        }
        let bounds = mux.pane_bounds(ws).unwrap();
        assert_eq!(
            crate::direction::pane_in_direction(&bounds, current_pane, PaneDirection::Left, |_| 0),
            Some(second_last_pane)
        );
    }

    #[test]
    fn tiebreak_prefers_most_recent_active_point() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let tl = mux.active_pane(ws).unwrap();
        let r = split_after(&mut mux, tl, SplitOrientation::Horizontal);
        let bl = split_after(&mut mux, tl, SplitOrientation::Vertical);
        let bounds = mux.pane_bounds(ws).unwrap();

        let scores_tl_higher = |p: PaneId| {
            if p == tl {
                2u64
            } else if p == bl {
                1
            } else {
                0
            }
        };
        assert_eq!(
            crate::direction::pane_in_direction(&bounds, r, PaneDirection::Left, scores_tl_higher),
            Some(tl),
            "tl has higher score so wins tiebreak"
        );

        let scores_bl_higher = |p: PaneId| {
            if p == bl {
                2u64
            } else if p == tl {
                1
            } else {
                0
            }
        };
        assert_eq!(
            crate::direction::pane_in_direction(&bounds, r, PaneDirection::Left, scores_bl_higher),
            Some(bl)
        );
    }

    #[test]
    fn navigate_focuses_geometric_neighbor() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let left = mux.active_pane(ws).unwrap();
        let right = split_after(&mut mux, left, SplitOrientation::Horizontal);
        mux.focus_pane(left).unwrap();

        assert_eq!(
            mux.navigate(left, PaneDirection::Right).unwrap(),
            vec![MuxEvent::ActivePaneChanged {
                workspace: ws,
                pane: right,
            }]
        );
        assert_eq!(mux.active_pane(ws).unwrap(), right);
        assert_eq!(
            mux.navigate(right, PaneDirection::Down).unwrap(),
            vec![],
            "no neighbor downward in a 1x2 horizontal strip"
        );
    }

    fn swap_resolved(mux: &MultiPlexer, ws: WorkspaceId) -> Vec<(PaneId, (u16, u16))> {
        mux.resolve_sizes(ws, 120, 40).unwrap()
    }

    #[test]
    fn swap_pane_swaps_positions_and_slot_grows() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let a = mux.active_pane(ws).unwrap();
        let b = split_after(&mut mux, a, SplitOrientation::Horizontal);
        let split = root_split(&mux, ws);
        mux.set_workspace_size(ws, 120, 40).unwrap();
        // Distinct ratio so slot-pinning is observable: slot A (first) gets 3/4.
        mux.splits[split].set_ratio(0.75);
        let before = swap_resolved(&mux, ws);

        let events = mux.swap_pane(a, SwapOffset::Next).unwrap();

        assert_eq!(mux.splits[split].first, NodeId::Pane(b));
        assert_eq!(mux.splits[split].second, NodeId::Pane(a));
        assert_eq!(mux.panes[a].parent, Some(NodeId::Split(split)));
        assert_eq!(mux.panes[b].parent, Some(NodeId::Split(split)));
        assert_eq!(mux.splits[split].ratio(), 0.75, "ratio stays on the slot");

        let after = swap_resolved(&mux, ws);
        // Slot A's size (90 cols at ratio 0.75 of 120) now belongs to `b`; B's to `a`.
        let size =
            |pane, list: &[(PaneId, (u16, u16))]| list.iter().find(|(p, _)| *p == pane).unwrap().1;
        assert_eq!(size(a, &before), (90, 40));
        assert_eq!(size(b, &after), (90, 40), "b inherited slot A's size");
        assert_eq!(size(a, &after), (30, 40), "a inherited slot B's size");

        assert!(
            events.iter().any(|e| matches!(
                e,
                MuxEvent::LayoutChanged { root, .. }
                    if matches!(root, NodeId::Split(s) if *s == split)
            )),
            "a LayoutChanged is emitted for the affected split: {events:?}"
        );
    }

    #[test]
    fn swap_pane_single_pane_is_noop() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        assert_eq!(mux.swap_pane(pane, SwapOffset::Next).unwrap(), vec![]);
    }

    fn two_panes_h(cols: u16, rows: u16) -> (MultiPlexer, WorkspaceId, PaneId, PaneId, SplitId) {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let left = mux.active_pane(ws).unwrap();
        let right = split_after(&mut mux, left, SplitOrientation::Horizontal);
        let split = root_split(&mux, ws);
        mux.set_workspace_size(ws, cols, rows).unwrap();
        (mux, ws, left, right, split)
    }

    #[test]
    fn resize_right_grows_lhs_shrinks_rhs() {
        let (mut mux, _ws, left, _right, split) = two_panes_h(120, 40);
        let before = mux.splits[split].ratio();
        let events = mux.resize_pane(left, PaneDirection::Right, 1).unwrap();
        let after = mux.splits[split].ratio();
        assert!(after > before, "lhs grew: {after} > {before}");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, MuxEvent::LayoutRatioChanged { .. })),
            "an applied resize emits LayoutRatioChanged: {events:?}"
        );
    }

    #[test]
    fn resize_no_matching_ancestor_is_noop() {
        let (mut mux, _ws, left, _right, _split) = two_panes_h(120, 40);
        assert_eq!(
            mux.resize_pane(left, PaneDirection::Down, 1).unwrap(),
            vec![]
        );
    }

    #[test]
    fn resize_clamps_at_min_cells_when_shrinking_subtree_is_at_floor() {
        let (mut mux, _ws, left, _right, split) = two_panes_h(120, 40);
        // Push rhs to its 10-col floor (110/10 of 120) so Right has no budget.
        mux.splits[split].set_ratio(110.0 / 120.0);
        assert_eq!(
            mux.resize_pane(left, PaneDirection::Right, 5).unwrap(),
            vec![]
        );
    }

    #[test]
    fn resize_partially_applies_when_amount_exceeds_available_budget() {
        let (mut mux, _ws, left, _right, split) = two_panes_h(120, 40);
        let events = mux.resize_pane(left, PaneDirection::Right, 100).unwrap();
        assert!(!events.is_empty(), "resize applied");
        let ratio = mux.splits[split].ratio();
        assert!(
            (ratio - 110.0 / 120.0).abs() < 1e-6,
            "lhs fraction ~ 110/120, got {ratio}"
        );
    }

    #[test]
    fn resize_in_2x2_grid_resolves_cross_axis_and_same_axis_ancestors() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let p1 = mux.active_pane(ws).unwrap();
        let p2 = split_after(&mut mux, p1, SplitOrientation::Horizontal);
        let _p3 = split_after(&mut mux, p1, SplitOrientation::Vertical);
        let _p4 = split_after(&mut mux, p2, SplitOrientation::Vertical);
        mux.set_workspace_size(ws, 120, 40).unwrap();
        let outer = root_split(&mux, ws);
        let before = mux.splits[outer].ratio();

        let ev = mux.resize_pane(p1, PaneDirection::Right, 5).unwrap();
        assert!(!ev.is_empty());
        assert!(
            mux.splits[outer].ratio() > before,
            "outer (cross-axis-walked) split's lhs grew"
        );

        assert!(
            !mux.resize_pane(p1, PaneDirection::Down, 3)
                .unwrap()
                .is_empty(),
            "Down matches the inner same-axis SplitV ancestor"
        );
    }

    #[test]
    fn resize_clamps_via_recursive_min_check_in_same_axis_chain() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let p1 = mux.active_pane(ws).unwrap();
        let p2 = split_after(&mut mux, p1, SplitOrientation::Horizontal);
        let p3 = split_after(&mut mux, p2, SplitOrientation::Horizontal);
        mux.set_workspace_size(ws, 120, 40).unwrap();

        let mut terminated = false;
        let mut iterations = 0;
        for _ in 0..200 {
            iterations += 1;
            if mux
                .resize_pane(p1, PaneDirection::Right, 1)
                .unwrap()
                .is_empty()
            {
                terminated = true;
                break;
            }
        }
        assert!(
            terminated && iterations < 200,
            "growth must clamp to NoOp within 200 iterations; ran {iterations}"
        );
        assert!(
            mux.resize_pane(p1, PaneDirection::Right, 50)
                .unwrap()
                .is_empty(),
            "already at the floor: a large further grow is a NoOp"
        );
        assert!(mux.panes.get(p2).is_some(), "p2 leaf present after clamp");
        assert!(mux.panes.get(p3).is_some(), "p3 leaf present after clamp");
    }

    #[test]
    fn resize_no_drift_across_repeated_one_cell_adjustments() {
        let (mut mux, _ws, left, _right, split) = two_panes_h(120, 40);
        mux.resize_pane(left, PaneDirection::Right, 1).unwrap();
        mux.resize_pane(left, PaneDirection::Left, 1).unwrap();
        let before = mux.splits[split].ratio();
        for _ in 0..50 {
            mux.resize_pane(left, PaneDirection::Right, 1).unwrap();
            mux.resize_pane(left, PaneDirection::Left, 1).unwrap();
        }
        let after = mux.splits[split].ratio();
        assert!(
            (before - after).abs() < 1e-3,
            "ratio drift: {before} -> {after}"
        );
    }

    #[test]
    fn resize_pane_returns_noop_without_workspace_size() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let p1 = mux.active_pane(ws).unwrap();
        let _p2 = split_after(&mut mux, p1, SplitOrientation::Horizontal);
        assert_eq!(
            mux.resize_pane(p1, PaneDirection::Right, 5).unwrap(),
            vec![]
        );
    }

    #[test]
    fn resize_pane_returns_noop_for_single_pane_workspace() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let p = mux.active_pane(ws).unwrap();
        mux.set_workspace_size(ws, 120, 40).unwrap();
        assert_eq!(mux.resize_pane(p, PaneDirection::Right, 5).unwrap(), vec![]);
    }

    #[test]
    fn set_workspace_size_emits_resized_for_changed_panes() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let left = mux.active_pane(ws).unwrap();
        let right = split_after(&mut mux, left, SplitOrientation::Horizontal);

        let events = mux.set_workspace_size(ws, 120, 40).unwrap();
        assert!(events.contains(&MuxEvent::PaneResized {
            pane: left,
            cols: 60,
            rows: 40,
        }));
        assert!(events.contains(&MuxEvent::PaneResized {
            pane: right,
            cols: 60,
            rows: 40,
        }));

        let re_events = mux.set_workspace_size(ws, 120, 40).unwrap();
        assert!(
            !re_events
                .iter()
                .any(|e| matches!(e, MuxEvent::PaneResized { .. })),
            "re-setting the same size emits no PaneResized"
        );
        assert!(
            !re_events
                .iter()
                .any(|e| matches!(e, MuxEvent::WorkspaceResized { .. })),
            "re-setting the same size emits no WorkspaceResized"
        );

        let changed_events = mux.set_workspace_size(ws, 100, 30).unwrap();
        assert!(
            changed_events
                .iter()
                .any(|e| matches!(e, MuxEvent::WorkspaceResized { workspace: w, cols: 100, rows: 30 } if *w == ws)),
            "a new size emits WorkspaceResized"
        );
    }

    #[test]
    fn resolved_pane_size_is_some_when_sized_none_when_unsized() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        assert_eq!(
            mux.resolved_pane_size(pane),
            None,
            "unsized workspace → None"
        );
        mux.set_workspace_size(ws, 80, 24).unwrap();
        assert_eq!(
            mux.resolved_pane_size(pane),
            Some((80, 24)),
            "the sole pane fills the 80x24 workspace"
        );
    }

    #[test]
    fn create_workspace_spawns_root_pane_surface_tree() {
        let mut mux = MultiPlexer::new();
        let events = mux.new_workspace(None).unwrap();

        let (session, workspace) = match events[0] {
            MuxEvent::WorkspaceCreated {
                session, workspace, ..
            } => (session, workspace),
            _ => panic!("first event must be WorkspaceCreated"),
        };
        let pane = match &events[1] {
            MuxEvent::PaneCreated {
                pane,
                workspace: ws,
                surfaces,
                ..
            } => {
                assert_eq!(*ws, workspace);
                assert_eq!(surfaces.len(), 1);
                assert!(matches!(surfaces[0].kind, SurfaceKind::Terminal));
                *pane
            }
            _ => panic!("second event must be PaneCreated"),
        };
        assert_eq!(
            events[2],
            MuxEvent::WorkspaceSelected { session, workspace },
        );
        assert_eq!(events[3], MuxEvent::ActivePaneChanged { workspace, pane },);

        assert_eq!(mux.active_workspace(), workspace);
        assert_eq!(mux.active_pane(workspace).unwrap(), pane);
        let surfaces = mux.surfaces(pane).unwrap();
        assert_eq!(surfaces.len(), 1);
        assert!(matches!(
            mux.surface_kind(surfaces[0]).unwrap(),
            SurfaceKind::Terminal
        ));
    }

    #[test]
    fn rename_workspace_mutates_name_and_only_fires_changed_on_actual_change() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();

        let events = mux.rename_workspace(ws, "new-name".to_string()).unwrap();
        assert_eq!(
            events,
            vec![MuxEvent::WorkspaceRenamed {
                workspace: ws,
                name: "new-name".to_string(),
            }],
            "rename to a new name emits WorkspaceRenamed"
        );

        let no_change = mux.rename_workspace(ws, "new-name".to_string()).unwrap();
        assert_eq!(no_change, vec![], "rename to same name emits nothing");
    }

    #[test]
    fn close_workspace_despawns_workspace_and_descendants() {
        let mut mux = MultiPlexer::new();
        let first_ws = mux.active_workspace();
        let evs = mux.new_workspace(None).unwrap();
        let second_ws = match evs[0] {
            MuxEvent::WorkspaceCreated { workspace, .. } => workspace,
            _ => panic!("WorkspaceCreated expected"),
        };
        let second_pane = mux.active_pane(second_ws).unwrap();
        let second_surface = mux.active_surface(second_pane).unwrap();

        mux.close_workspace(second_ws).unwrap();

        assert!(
            matches!(
                mux.workspace(second_ws),
                Err(MuxError::WorkspaceNotFound(id)) if id == second_ws
            ),
            "workspace slotmap entry removed"
        );
        assert_eq!(
            mux.pane(second_pane),
            Err(MuxError::PaneNotFound(second_pane)),
            "pane cascade-removed"
        );
        assert_eq!(
            mux.surface(second_surface),
            Err(MuxError::SurfaceNotFound(second_surface)),
            "surface cascade-removed"
        );
        assert_eq!(
            mux.active_workspace(),
            first_ws,
            "active re-points to the remaining workspace after closing the active one"
        );
    }

    #[test]
    fn close_workspace_frees_interior_splits() {
        let mut mux = MultiPlexer::new();
        mux.new_workspace(None).unwrap();
        let ws1 = mux.active_workspace();
        let p1 = mux.active_pane(ws1).unwrap();
        // Build SplitA(SplitB(p1, p1b), SplitC(p2, p2b)) — SplitA is interior
        // (both children are splits), the case the buggy leaf-parent loop leaked.
        mux.split_pane(
            p1,
            SplitOrientation::Horizontal,
            Side::After,
            SurfaceKind::Terminal,
            None,
        )
        .unwrap();
        let panes = mux.ordered_panes(ws1).unwrap();
        let (left, right) = (panes[0], panes[1]);
        mux.split_pane(
            left,
            SplitOrientation::Vertical,
            Side::After,
            SurfaceKind::Terminal,
            None,
        )
        .unwrap();
        mux.split_pane(
            right,
            SplitOrientation::Vertical,
            Side::After,
            SurfaceKind::Terminal,
            None,
        )
        .unwrap();
        assert_eq!(mux.ordered_panes(ws1).unwrap().len(), 4);
        assert_eq!(mux.splits.len(), 3, "3 splits: interior A + B + C");

        mux.close_workspace(ws1).unwrap();
        assert_eq!(
            mux.splits.len(),
            0,
            "all splits freed, including the interior one"
        );
        assert!(
            mux.panes.len() <= 1,
            "only the replacement workspace's pane remains"
        );
    }

    #[test]
    fn select_workspace_changes_active_and_is_changed_only() {
        let mut mux = MultiPlexer::new();
        let first_ws = mux.active_workspace();
        let evs = mux.new_workspace(None).unwrap();
        let second_ws = match evs[0] {
            MuxEvent::WorkspaceCreated { workspace, .. } => workspace,
            _ => panic!("WorkspaceCreated expected"),
        };
        // new_workspace already made second_ws active.
        let noop = mux.select_workspace(second_ws).unwrap();
        assert_eq!(
            noop,
            vec![],
            "selecting the already-active workspace emits nothing"
        );
        let changed = mux.select_workspace(first_ws).unwrap();
        assert!(
            changed.iter().any(
                |e| matches!(e, MuxEvent::WorkspaceSelected { workspace, .. } if *workspace == first_ws)
            ),
            "selecting a different workspace emits WorkspaceSelected"
        );
        assert_eq!(mux.active_workspace(), first_ws);
    }

    #[test]
    fn break_surface_to_pane_preserves_moved_surface_kind() {
        use std::path::PathBuf;
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let ext = SurfaceKind::Extension {
            entry: PathBuf::from("index.html"),
        };
        let spawn = mux.spawn_surface(pane, ext.clone(), None).unwrap();
        let ext_surface = match spawn[0] {
            MuxEvent::SurfaceSpawned { surface, .. } => surface,
            _ => panic!("SurfaceSpawned expected"),
        };
        let events = mux
            .break_surface_to_pane(ext_surface, SplitOrientation::Horizontal, Side::After)
            .unwrap();
        let new_pane = match &events[0] {
            MuxEvent::PaneCreated {
                pane: new_pane,
                surfaces,
                active_surface,
                ..
            } => {
                assert_eq!(surfaces.len(), 1, "PaneCreated carries exactly one surface");
                assert_eq!(
                    surfaces[0].kind, ext,
                    "PaneCreated carries the MOVED surface's kind, not Terminal"
                );
                assert_eq!(
                    surfaces[0].surface, *active_surface,
                    "the moved surface is the active surface"
                );
                *new_pane
            }
            _ => panic!("PaneCreated expected first"),
        };
        let surface_moved = events
            .iter()
            .find(|e| matches!(e, MuxEvent::SurfaceMoved { .. }));
        match surface_moved {
            Some(MuxEvent::SurfaceMoved {
                surface,
                from_pane,
                to_pane,
            }) => {
                assert_eq!(
                    *surface, ext_surface,
                    "SurfaceMoved names the moved surface"
                );
                assert_eq!(*from_pane, pane, "SurfaceMoved names the source pane");
                assert_eq!(
                    *to_pane, new_pane,
                    "SurfaceMoved names the destination pane"
                );
            }
            _ => panic!("SurfaceMoved expected in event list"),
        }
    }

    #[test]
    fn add_surface_spawns_surface_child_of_pane() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let original_active = mux.active_surface(pane).unwrap();

        let events = mux
            .spawn_surface(pane, SurfaceKind::Terminal, None)
            .unwrap();
        let new_surface = match events[0] {
            MuxEvent::SurfaceSpawned {
                surface,
                pane: p,
                ref kind,
                ..
            } => {
                assert_eq!(p, pane);
                assert!(matches!(kind, SurfaceKind::Terminal));
                surface
            }
            _ => panic!("SurfaceSpawned expected"),
        };

        assert!(
            mux.surfaces(pane).unwrap().contains(&new_surface),
            "new surface appears in pane's surfaces"
        );
        assert_eq!(
            mux.active_surface(pane).unwrap(),
            original_active,
            "active surface unchanged"
        );
    }

    #[test]
    fn add_surface_stamps_surfaceof_and_appears_in_surfaces() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let original = mux.active_surface(pane).unwrap();

        mux.spawn_surface(pane, SurfaceKind::Terminal, None)
            .unwrap();

        let surfaces = mux.surfaces(pane).unwrap();
        assert!(
            surfaces.contains(&original),
            "original surface still present"
        );
        assert_eq!(surfaces.len(), 2, "pane has two surfaces after spawn");
    }

    #[test]
    fn break_surface_to_pane_creates_new_pane_with_moved_surface() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let source_pane = mux.active_pane(ws).unwrap();

        mux.spawn_surface(source_pane, SurfaceKind::Terminal, None)
            .unwrap();
        let second_surface = mux.surfaces(source_pane).unwrap()[1];

        let events = mux
            .break_surface_to_pane(second_surface, SplitOrientation::Horizontal, Side::After)
            .unwrap();

        let new_pane = match events[0] {
            MuxEvent::PaneCreated { pane, .. } => pane,
            _ => panic!("first event must be PaneCreated"),
        };

        assert_eq!(
            mux.surfaces(new_pane).unwrap(),
            vec![second_surface],
            "moved surface is sole surface of new pane"
        );
        assert_eq!(
            mux.active_surface(new_pane).unwrap(),
            second_surface,
            "new pane's active_surface is the moved surface"
        );
        assert!(mux.pane(source_pane).is_ok(), "source pane still exists");
        assert!(
            !mux.surfaces(source_pane).unwrap().contains(&second_surface),
            "moved surface removed from source pane"
        );

        let surface_moved = events
            .iter()
            .find(|e| matches!(e, MuxEvent::SurfaceMoved { .. }));
        match surface_moved {
            Some(MuxEvent::SurfaceMoved {
                surface,
                from_pane,
                to_pane,
            }) => {
                assert_eq!(
                    *surface, second_surface,
                    "SurfaceMoved names the moved surface"
                );
                assert_eq!(
                    *from_pane, source_pane,
                    "SurfaceMoved names the source pane"
                );
                assert_eq!(
                    *to_pane, new_pane,
                    "SurfaceMoved names the destination pane"
                );
            }
            _ => panic!("SurfaceMoved expected in event list"),
        }
    }

    #[test]
    fn break_surface_to_pane_returns_error_when_source_pane_has_only_one_surface() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let surface = mux.active_surface(pane).unwrap();

        let result = mux.break_surface_to_pane(surface, SplitOrientation::Horizontal, Side::After);
        assert!(
            matches!(result, Err(MuxError::CannotRemoveLastSurface(_))),
            "expected CannotRemoveLastSurface, got {result:?}"
        );
    }

    #[test]
    fn close_surface_removes_surface_and_repoints_active() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let s1 = mux.active_surface(pane).unwrap();
        mux.spawn_surface(pane, SurfaceKind::Terminal, None)
            .unwrap();
        let s2 = mux.surfaces(pane).unwrap()[1];

        mux.set_active_surface(pane, s2).unwrap();
        let events = mux.close_surface(s2).unwrap();

        assert_eq!(
            mux.surface(s2),
            Err(MuxError::SurfaceNotFound(s2)),
            "closed surface removed from slotmap"
        );
        assert_eq!(
            mux.active_surface(pane).unwrap(),
            s1,
            "active re-pointed to s1"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, MuxEvent::SurfaceClosed { surface } if *surface == s2))
        );
        assert!(
            events.iter().any(|e| matches!(e, MuxEvent::ActiveSurfaceChanged { pane: p, surface } if *p == pane && *surface == s1))
        );
    }

    #[test]
    fn close_surface_last_surface_errors() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let surface = mux.active_surface(pane).unwrap();

        assert_eq!(
            mux.close_surface(surface),
            Err(MuxError::CannotRemoveLastSurface(pane))
        );
    }

    #[test]
    fn workspace_layout_serde_round_trips() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        mux.split_pane(
            pane,
            SplitOrientation::Horizontal,
            Side::After,
            SurfaceKind::Terminal,
            None,
        )
        .unwrap();
        mux.set_workspace_size(ws, 80, 24).unwrap();
        let layout = mux.workspace_layout(ws).unwrap();
        let json = serde_json::to_string(&layout).unwrap();
        let back: LayoutNode = serde_json::from_str(&json).unwrap();
        assert_eq!(layout, back);
    }

    #[test]
    fn set_surface_cwd_changed_only() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let surface = mux.active_surface(pane).unwrap();

        let path = PathBuf::from("/home/user");
        let events = mux.set_surface_cwd(surface, path.clone()).unwrap();
        assert_eq!(
            events,
            vec![MuxEvent::SurfaceCwdChanged {
                surface,
                cwd: path.clone(),
            }],
            "first set emits SurfaceCwdChanged"
        );

        let no_change = mux.set_surface_cwd(surface, path).unwrap();
        assert_eq!(no_change, vec![], "same cwd emits nothing");
    }

    #[test]
    fn set_surface_cwd_ignores_empty_path() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let surface = mux.active_surface(pane).unwrap();

        let path = PathBuf::from("/home/user");
        mux.set_surface_cwd(surface, path.clone()).unwrap();

        let empty = mux.set_surface_cwd(surface, PathBuf::new()).unwrap();
        assert_eq!(
            empty,
            vec![],
            "empty cwd is the no-cwd sentinel and emits nothing"
        );

        let again = mux.set_surface_cwd(surface, path).unwrap();
        assert_eq!(
            again,
            vec![],
            "the empty call must not overwrite the stored cwd"
        );
    }

    #[test]
    fn spawn_surface_with_cwd_seeds_surface_and_event() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let cwd = PathBuf::from("/tmp");

        let events = mux
            .spawn_surface(pane, SurfaceKind::Terminal, Some(cwd.clone()))
            .unwrap();

        let (spawned_surface, event_cwd) = match &events[0] {
            MuxEvent::SurfaceSpawned {
                surface, cwd: ec, ..
            } => (*surface, ec.clone()),
            _ => panic!("expected SurfaceSpawned"),
        };
        assert_eq!(
            event_cwd, cwd,
            "SurfaceSpawned.cwd must equal the seeded cwd"
        );

        let stored = &mux.surfaces[spawned_surface];
        assert_eq!(
            stored.cwd,
            Some(cwd),
            "Surface.cwd must be seeded from spawn_surface argument"
        );
    }

    #[test]
    fn split_pane_with_cwd_seeds_surface_and_pane_created_entry() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let cwd = PathBuf::from("/workdir");

        let events = mux
            .split_pane(
                pane,
                SplitOrientation::Horizontal,
                Side::After,
                SurfaceKind::Terminal,
                Some(cwd.clone()),
            )
            .unwrap();

        let surface_entry_cwd = match &events[0] {
            MuxEvent::PaneCreated { surfaces, .. } => surfaces[0].cwd.clone(),
            _ => panic!("expected PaneCreated"),
        };
        assert_eq!(
            surface_entry_cwd, cwd,
            "PaneCreated SurfaceEntry.cwd must equal the seeded cwd"
        );
    }

    #[test]
    fn active_session_returns_the_seeded_session() {
        let mux = MultiPlexer::new();
        assert_eq!(mux.active_session(), mux.sessions()[0]);
    }

    #[test]
    fn set_active_surface_by_surface_resolves_pane_and_switches() {
        let mut mux = MultiPlexer::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        let spawn = mux
            .spawn_surface(pane, SurfaceKind::Terminal, None)
            .unwrap();
        let new_surface = match spawn[0] {
            MuxEvent::SurfaceSpawned { surface, .. } => surface,
            _ => panic!("first event must be SurfaceSpawned"),
        };
        let events = mux.set_active_surface_by_surface(new_surface).unwrap();
        assert_eq!(
            events,
            vec![MuxEvent::ActiveSurfaceChanged {
                pane,
                surface: new_surface
            }]
        );
        assert_eq!(mux.active_surface(pane).unwrap(), new_surface);
    }

    #[test]
    fn set_active_surface_by_surface_unknown_is_surface_not_found() {
        let mut mux = MultiPlexer::new();
        assert_eq!(
            mux.set_active_surface_by_surface(SurfaceId::default()),
            Err(MuxError::SurfaceNotFound(SurfaceId::default()))
        );
    }
}
