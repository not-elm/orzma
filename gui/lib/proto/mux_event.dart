import 'ids.dart';
import 'layout.dart';

/// One surface in a pane's creation manifest.
class SurfaceEntry {
  final SurfaceId surface;
  final SurfaceKind kind;
  final String cwd;
  SurfaceEntry(this.surface, this.kind, this.cwd);
  factory SurfaceEntry.fromJson(Map<String, dynamic> j) => SurfaceEntry(
      SurfaceId.fromJson(j['surface'] as Map<String, dynamic>),
      SurfaceKind.fromJson(j['kind']),
      j['cwd'] as String);
}

/// A single multiplexer state-change event (mirror of `ozmux_mux::MuxEvent`).
sealed class MuxEvent {
  const MuxEvent();
  static MuxEvent fromJson(Object j) {
    if (j is String) return UnknownEvent(j);
    final m = j as Map<String, dynamic>;
    final key = m.keys.first;
    final v = m[key];
    switch (key) {
      case 'SessionCreated':
        return const SessionCreatedEvent();
      case 'WorkspaceCreated':
        return WorkspaceCreated(
            WorkspaceId.fromJson(v['workspace'] as Map<String, dynamic>),
            v['name'] as String);
      case 'WorkspaceDestroyed':
        return WorkspaceDestroyed(
            WorkspaceId.fromJson(v['workspace'] as Map<String, dynamic>));
      case 'WorkspaceSelected':
        return WorkspaceSelected(
            WorkspaceId.fromJson(v['workspace'] as Map<String, dynamic>));
      case 'WorkspaceRenamed':
        return WorkspaceRenamed(
            WorkspaceId.fromJson(v['workspace'] as Map<String, dynamic>),
            v['name'] as String);
      case 'PaneCreated':
        return PaneCreated(
          PaneId.fromJson(v['pane'] as Map<String, dynamic>),
          WorkspaceId.fromJson(v['workspace'] as Map<String, dynamic>),
          (v['surfaces'] as List)
              .map((e) => SurfaceEntry.fromJson(e as Map<String, dynamic>))
              .toList(),
          SurfaceId.fromJson(v['active_surface'] as Map<String, dynamic>),
        );
      case 'PaneClosed':
        return PaneClosed(PaneId.fromJson(v['pane'] as Map<String, dynamic>));
      case 'ActivePaneChanged':
        return ActivePaneChanged(
            WorkspaceId.fromJson(v['workspace'] as Map<String, dynamic>),
            PaneId.fromJson(v['pane'] as Map<String, dynamic>));
      case 'LayoutChanged':
        return LayoutChanged(
            WorkspaceId.fromJson(v['workspace'] as Map<String, dynamic>),
            NodeId.fromJson(v['root'] as Map<String, dynamic>),
            LayoutNode.fromJson(v['subtree'] as Map<String, dynamic>));
      case 'WorkspaceRootChanged':
        return WorkspaceRootChanged(
            WorkspaceId.fromJson(v['workspace'] as Map<String, dynamic>),
            LayoutNode.fromJson(v['root'] as Map<String, dynamic>));
      case 'LayoutRatioChanged':
        return LayoutRatioChanged(
            SplitId.fromJson(v['split'] as Map<String, dynamic>),
            (v['ratio'] as num).toDouble());
      case 'WorkspaceResized':
        return WorkspaceResized(
            WorkspaceId.fromJson(v['workspace'] as Map<String, dynamic>),
            v['cols'] as int, v['rows'] as int);
      case 'PaneResized':
        return PaneResized(PaneId.fromJson(v['pane'] as Map<String, dynamic>),
            v['cols'] as int, v['rows'] as int);
      case 'SurfaceSpawned':
        return SurfaceSpawned(
            PaneId.fromJson(v['pane'] as Map<String, dynamic>),
            SurfaceId.fromJson(v['surface'] as Map<String, dynamic>),
            SurfaceKind.fromJson(v['kind']),
            v['cwd'] as String);
      case 'SurfaceClosed':
        return SurfaceClosed(
            SurfaceId.fromJson(v['surface'] as Map<String, dynamic>));
      case 'ActiveSurfaceChanged':
        return ActiveSurfaceChanged(
            PaneId.fromJson(v['pane'] as Map<String, dynamic>),
            SurfaceId.fromJson(v['surface'] as Map<String, dynamic>));
      case 'SurfaceCwdChanged':
        return SurfaceCwdChanged(
            SurfaceId.fromJson(v['surface'] as Map<String, dynamic>),
            v['cwd'] as String);
      case 'SurfaceMoved':
        return SurfaceMoved(
            SurfaceId.fromJson(v['surface'] as Map<String, dynamic>),
            PaneId.fromJson(v['from_pane'] as Map<String, dynamic>),
            PaneId.fromJson(v['to_pane'] as Map<String, dynamic>));
      default:
        return UnknownEvent(key);
    }
  }
}

/// A new session was created.
class SessionCreatedEvent extends MuxEvent { const SessionCreatedEvent(); }
/// A workspace was created.
class WorkspaceCreated extends MuxEvent { final WorkspaceId workspace; final String name; const WorkspaceCreated(this.workspace, this.name); }
/// A workspace was destroyed.
class WorkspaceDestroyed extends MuxEvent { final WorkspaceId workspace; const WorkspaceDestroyed(this.workspace); }
/// The active workspace changed.
class WorkspaceSelected extends MuxEvent { final WorkspaceId workspace; const WorkspaceSelected(this.workspace); }
/// A workspace was renamed.
class WorkspaceRenamed extends MuxEvent { final WorkspaceId workspace; final String name; const WorkspaceRenamed(this.workspace, this.name); }
/// A pane was created with its surface manifest.
class PaneCreated extends MuxEvent { final PaneId pane; final WorkspaceId workspace; final List<SurfaceEntry> surfaces; final SurfaceId activeSurface; const PaneCreated(this.pane, this.workspace, this.surfaces, this.activeSurface); }
/// A pane was closed.
class PaneClosed extends MuxEvent { final PaneId pane; const PaneClosed(this.pane); }
/// The focused pane changed.
class ActivePaneChanged extends MuxEvent { final WorkspaceId workspace; final PaneId pane; const ActivePaneChanged(this.workspace, this.pane); }
/// A subtree of the layout was replaced.
class LayoutChanged extends MuxEvent { final WorkspaceId workspace; final NodeId root; final LayoutNode subtree; const LayoutChanged(this.workspace, this.root, this.subtree); }
/// The whole workspace layout root was replaced.
class WorkspaceRootChanged extends MuxEvent { final WorkspaceId workspace; final LayoutNode root; const WorkspaceRootChanged(this.workspace, this.root); }
/// A split's ratio changed.
class LayoutRatioChanged extends MuxEvent { final SplitId split; final double ratio; const LayoutRatioChanged(this.split, this.ratio); }
/// A workspace's total size changed.
class WorkspaceResized extends MuxEvent { final WorkspaceId workspace; final int cols; final int rows; const WorkspaceResized(this.workspace, this.cols, this.rows); }
/// A pane's resolved size changed.
class PaneResized extends MuxEvent { final PaneId pane; final int cols; final int rows; const PaneResized(this.pane, this.cols, this.rows); }
/// A surface was added to a pane.
class SurfaceSpawned extends MuxEvent { final PaneId pane; final SurfaceId surface; final SurfaceKind kind; final String cwd; const SurfaceSpawned(this.pane, this.surface, this.kind, this.cwd); }
/// A surface was removed.
class SurfaceClosed extends MuxEvent { final SurfaceId surface; const SurfaceClosed(this.surface); }
/// A pane's focused surface changed.
class ActiveSurfaceChanged extends MuxEvent { final PaneId pane; final SurfaceId surface; const ActiveSurfaceChanged(this.pane, this.surface); }
/// A surface's working directory changed.
class SurfaceCwdChanged extends MuxEvent { final SurfaceId surface; final String cwd; const SurfaceCwdChanged(this.surface, this.cwd); }
/// A surface moved from one pane to another.
class SurfaceMoved extends MuxEvent { final SurfaceId surface; final PaneId fromPane; final PaneId toPane; const SurfaceMoved(this.surface, this.fromPane, this.toPane); }
/// An unrecognized event tag (forward-compat).
class UnknownEvent extends MuxEvent { final String tag; const UnknownEvent(this.tag); }
