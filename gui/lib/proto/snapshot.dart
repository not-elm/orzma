import 'package:json_annotation/json_annotation.dart';
import 'ids.dart';
import 'layout.dart';

part 'snapshot.g.dart';

// --- fromJson converters: bridge json_serializable to our hand-written types ---
WorkspaceId _wsId(Map<String, dynamic> j) => WorkspaceId.fromJson(j);
PaneId _paneId(Map<String, dynamic> j) => PaneId.fromJson(j);
SurfaceId _surfId(Map<String, dynamic> j) => SurfaceId.fromJson(j);
SessionId _sessId(Map<String, dynamic> j) => SessionId.fromJson(j);
SurfaceKind _kind(dynamic j) => SurfaceKind.fromJson(j);
LayoutNode _layout(Map<String, dynamic> j) => LayoutNode.fromJson(j);

/// One surface's state (kind + working directory).
@JsonSerializable(createToJson: false)
class SurfaceState {
  @JsonKey(fromJson: _surfId)
  final SurfaceId surface;
  @JsonKey(fromJson: _kind)
  final SurfaceKind kind;
  final String cwd;
  SurfaceState({required this.surface, required this.kind, required this.cwd});
  factory SurfaceState.fromJson(Map<String, dynamic> j) =>
      _$SurfaceStateFromJson(j);
}

/// One pane's surfaces + its active surface.
@JsonSerializable(createToJson: false)
class PaneSnapshot {
  @JsonKey(fromJson: _paneId)
  final PaneId pane;
  final List<SurfaceState> surfaces;
  @JsonKey(name: 'active_surface', fromJson: _surfId)
  final SurfaceId activeSurface;
  PaneSnapshot(
      {required this.pane, required this.surfaces, required this.activeSurface});
  factory PaneSnapshot.fromJson(Map<String, dynamic> j) =>
      _$PaneSnapshotFromJson(j);
}

/// One workspace's layout tree + panes.
@JsonSerializable(createToJson: false)
class WorkspaceSnapshot {
  @JsonKey(fromJson: _wsId)
  final WorkspaceId workspace;
  final String name;
  @JsonKey(fromJson: _layout)
  final LayoutNode layout;
  @JsonKey(name: 'active_pane', fromJson: _paneId)
  final PaneId activePane;
  final List<PaneSnapshot> panes;
  WorkspaceSnapshot(
      {required this.workspace,
      required this.name,
      required this.layout,
      required this.activePane,
      required this.panes});
  factory WorkspaceSnapshot.fromJson(Map<String, dynamic> j) =>
      _$WorkspaceSnapshotFromJson(j);
}

/// A session's full state: workspaces + the active one.
@JsonSerializable(createToJson: false)
class SessionSnapshot {
  @JsonKey(fromJson: _sessId)
  final SessionId session;
  @JsonKey(name: 'active_workspace', fromJson: _wsId)
  final WorkspaceId activeWorkspace;
  final List<WorkspaceSnapshot> workspaces;
  SessionSnapshot(
      {required this.session,
      required this.activeWorkspace,
      required this.workspaces});
  factory SessionSnapshot.fromJson(Map<String, dynamic> j) =>
      _$SessionSnapshotFromJson(j);
}
