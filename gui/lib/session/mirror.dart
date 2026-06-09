import '../proto/ids.dart';
import '../proto/layout.dart';
import '../proto/mux_event.dart';
import '../proto/snapshot.dart';

/// A pane's surface (mutable working copy for the fold).
class MutableSurface {
  final SurfaceId surface;
  SurfaceKind kind;
  String cwd;
  MutableSurface(this.surface, this.kind, this.cwd);
}

/// A pane with its surfaces + active surface (mutable working copy).
class MutablePane {
  final PaneId pane;
  final List<MutableSurface> surfaces;
  SurfaceId activeSurface;
  MutablePane(this.pane, this.surfaces, this.activeSurface);
}

/// A workspace's layout tree + panes (mutable working copy).
class MutableWorkspace {
  final WorkspaceId workspace;
  String name;
  LayoutNode layout;
  PaneId activePane;
  final List<MutablePane> panes;
  MutableWorkspace({
    required this.workspace,
    required this.name,
    required this.layout,
    required this.activePane,
    required this.panes,
  });
}

/// The reconstructed session state the UI renders.
class SessionState {
  WorkspaceId activeWorkspace;
  final List<MutableWorkspace> workspaces;
  SessionState(this.activeWorkspace, this.workspaces);
}

/// The slotmap null-key sentinel used by the daemon for a placeholder pane id
/// (`PaneId::default()` == `{idx: u32::MAX, version: 1}`).
const PaneId _placeholderPane = PaneId(4294967295, 1);

/// Dart port of `ozmux_proto::ClientMirror` (see mirror.rs). Folds a Welcome
/// snapshot + `MuxEvent` batches into a [SessionState]. After any
/// layout-mutating event in a batch, it prunes panes no longer in the tree and
/// reorders the pane list into the tree's DFS leaf order — deferred to batch
/// end so a cross-parent swap (remove-then-re-add across events) is not pruned
/// mid-batch.
class ClientMirror {
  /// The reconstructed session state.
  final SessionState state;
  ClientMirror._(this.state);

  /// Builds a mirror from a cold-attach snapshot.
  factory ClientMirror.fromSnapshot(SessionSnapshot snap) {
    final wss = snap.workspaces
        .map((w) => MutableWorkspace(
              workspace: w.workspace,
              name: w.name,
              layout: w.layout,
              activePane: w.activePane,
              panes: w.panes
                  .map((p) => MutablePane(
                        p.pane,
                        p.surfaces
                            .map((s) => MutableSurface(s.surface, s.kind, s.cwd))
                            .toList(),
                        p.activeSurface,
                      ))
                  .toList(),
            ))
        .toList();
    return ClientMirror._(SessionState(snap.activeWorkspace, wss));
  }

  /// Folds an event batch, deferring pane prune/reorder to the end.
  void applyEvents(List<MuxEvent> events) {
    for (final e in events) {
      _applyNoPrune(e);
    }
    final layoutTouched =
        events.any((e) => e is LayoutChanged || e is WorkspaceRootChanged);
    if (layoutTouched) {
      for (final ws in state.workspaces) {
        _prunePanes(ws);
        _reorderPanesToLayout(ws);
      }
    }
  }

  /// Folds a single event (used by tests / single-event callers).
  void applyEvent(MuxEvent event) => applyEvents([event]);

  void _applyNoPrune(MuxEvent e) {
    switch (e) {
      case WorkspaceCreated(:final workspace, :final name):
        state.workspaces.add(MutableWorkspace(
          workspace: workspace,
          name: name,
          layout: const LayoutPane(id: _placeholderPane, surfaceKind: TerminalKind()),
          activePane: _placeholderPane,
          panes: [],
        ));
      case WorkspaceDestroyed(:final workspace):
        state.workspaces.removeWhere((w) => w.workspace == workspace);
      case WorkspaceSelected(:final workspace):
        state.activeWorkspace = workspace;
      case WorkspaceRenamed(:final workspace, :final name):
        final ws = _findWs(workspace);
        if (ws != null) ws.name = name;
      case PaneCreated(:final pane, :final workspace, :final surfaces, :final activeSurface):
        final ws = _findWs(workspace);
        if (ws != null) {
          // NOTE: an empty pane list means this is the workspace's root pane
          // (new_workspace emits no LayoutChanged) — establish the root layout
          // here. A split's PaneCreated arrives on a non-empty workspace and is
          // positioned by the following LayoutChanged; do not overwrite there.
          if (ws.panes.isEmpty) {
            final kind = surfaces
                    .where((s) => s.surface == activeSurface)
                    .map((s) => s.kind)
                    .followedBy(surfaces.map((s) => s.kind))
                    .cast<SurfaceKind?>()
                    .firstWhere((_) => true, orElse: () => null) ??
                const TerminalKind();
            ws.layout = LayoutPane(id: pane, surfaceKind: kind);
          }
          ws.panes.add(MutablePane(
            pane,
            surfaces.map((s) => MutableSurface(s.surface, s.kind, s.cwd)).toList(),
            activeSurface,
          ));
        }
      case PaneClosed(:final pane):
        for (final ws in state.workspaces) {
          ws.panes.removeWhere((p) => p.pane == pane);
        }
      case ActivePaneChanged(:final workspace, :final pane):
        final ws = _findWs(workspace);
        if (ws != null) ws.activePane = pane;
      case LayoutChanged(:final workspace, :final root, :final subtree):
        final ws = _findWs(workspace);
        if (ws != null) ws.layout = _applyLayoutNode(ws.layout, root, subtree);
      case WorkspaceRootChanged(:final workspace, :final root):
        final ws = _findWs(workspace);
        if (ws != null) ws.layout = root;
      case LayoutRatioChanged(:final split, :final ratio):
        for (final ws in state.workspaces) {
          ws.layout = _setSplitRatio(ws.layout, split, ratio);
        }
      case SurfaceSpawned(:final pane, :final surface, :final kind, :final cwd):
        final p = _findPane(pane);
        if (p != null) p.surfaces.add(MutableSurface(surface, kind, cwd));
      case SurfaceClosed(:final surface):
        for (final ws in state.workspaces) {
          for (final p in ws.panes) {
            p.surfaces.removeWhere((s) => s.surface == surface);
          }
        }
      case ActiveSurfaceChanged(:final pane, :final surface):
        final p = _findPane(pane);
        if (p != null) p.activeSurface = surface;
      case SurfaceCwdChanged(:final surface, :final cwd):
        for (final ws in state.workspaces) {
          for (final p in ws.panes) {
            for (final s in p.surfaces) {
              if (s.surface == surface) s.cwd = cwd;
            }
          }
        }
      // NOTE: PaneCreated (emitted first) already added `surface` to to_pane;
      // SurfaceMoved removes it from from_pane. Apply in emission order.
      case SurfaceMoved(:final surface, :final fromPane):
        final p = _findPane(fromPane);
        if (p != null) p.surfaces.removeWhere((s) => s.surface == surface);
      // PaneResized / WorkspaceResized / SessionCreated / Unknown: no-op
      // (this port does not track resolved cell sizes — see class doc).
      case PaneResized():
        break;
      case WorkspaceResized():
        break;
      case SessionCreatedEvent():
        break;
      case UnknownEvent():
        break;
    }
  }

  MutableWorkspace? _findWs(WorkspaceId id) {
    for (final ws in state.workspaces) {
      if (ws.workspace == id) return ws;
    }
    return null;
  }

  MutablePane? _findPane(PaneId id) {
    for (final ws in state.workspaces) {
      for (final p in ws.panes) {
        if (p.pane == id) return p;
      }
    }
    return null;
  }

  /// Replaces the node addressed by `target` (root or descendant) with `subtree`.
  LayoutNode _applyLayoutNode(LayoutNode layout, NodeId target, LayoutNode subtree) =>
      _tryReplace(layout, target, subtree) ?? layout;

  /// Returns a rebuilt tree with the `target` node replaced by `sub`, or null if
  /// `target` is not present in `tree`.
  LayoutNode? _tryReplace(LayoutNode tree, NodeId target, LayoutNode sub) {
    if (_nodeMatches(tree, target)) return sub;
    if (tree is LayoutSplit) {
      final nf = _tryReplace(tree.first, target, sub);
      if (nf != null) {
        return LayoutSplit(id: tree.id, orientation: tree.orientation, ratio: tree.ratio, first: nf, second: tree.second);
      }
      final ns = _tryReplace(tree.second, target, sub);
      if (ns != null) {
        return LayoutSplit(id: tree.id, orientation: tree.orientation, ratio: tree.ratio, first: tree.first, second: ns);
      }
    }
    return null;
  }

  bool _nodeMatches(LayoutNode node, NodeId target) =>
      (node is LayoutSplit && target is NodeSplit && node.id == target.id) ||
      (node is LayoutPane && target is NodePane && node.id == target.id);

  /// Rebuilds the tree with `target` split's ratio set (clamped to [0,1];
  /// non-finite rescued to 0.5, matching Mux's `Split::set_ratio`).
  LayoutNode _setSplitRatio(LayoutNode tree, SplitId target, double ratio) {
    if (tree is LayoutSplit) {
      if (tree.id == target) {
        final r = ratio.isFinite ? ratio.clamp(0.0, 1.0) : 0.5;
        return LayoutSplit(id: tree.id, orientation: tree.orientation, ratio: r, first: tree.first, second: tree.second);
      }
      return LayoutSplit(
        id: tree.id,
        orientation: tree.orientation,
        ratio: tree.ratio,
        first: _setSplitRatio(tree.first, target, ratio),
        second: _setSplitRatio(tree.second, target, ratio),
      );
    }
    return tree;
  }

  void _prunePanes(MutableWorkspace ws) {
    final live = <PaneId>{};
    _collectPaneIds(ws.layout, live);
    ws.panes.removeWhere((p) => !live.contains(p.pane));
  }

  void _collectPaneIds(LayoutNode node, Set<PaneId> out) {
    if (node is LayoutSplit) {
      _collectPaneIds(node.first, out);
      _collectPaneIds(node.second, out);
    } else if (node is LayoutPane) {
      out.add(node.id);
    }
  }

  void _reorderPanesToLayout(MutableWorkspace ws) {
    final order = <PaneId>[];
    _dfsLeafPanes(ws.layout, order);
    ws.panes.sort((a, b) {
      final ia = order.indexOf(a.pane);
      final ib = order.indexOf(b.pane);
      return (ia < 0 ? order.length : ia).compareTo(ib < 0 ? order.length : ib);
    });
  }

  void _dfsLeafPanes(LayoutNode node, List<PaneId> out) {
    if (node is LayoutSplit) {
      _dfsLeafPanes(node.first, out);
      _dfsLeafPanes(node.second, out);
    } else if (node is LayoutPane) {
      out.add(node.id);
    }
  }
}
