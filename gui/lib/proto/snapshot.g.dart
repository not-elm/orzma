// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'snapshot.dart';

// **************************************************************************
// JsonSerializableGenerator
// **************************************************************************

SurfaceState _$SurfaceStateFromJson(Map<String, dynamic> json) => SurfaceState(
  surface: _surfId(json['surface'] as Map<String, dynamic>),
  kind: _kind(json['kind']),
  cwd: json['cwd'] as String,
);

PaneSnapshot _$PaneSnapshotFromJson(Map<String, dynamic> json) => PaneSnapshot(
  pane: _paneId(json['pane'] as Map<String, dynamic>),
  surfaces: (json['surfaces'] as List<dynamic>)
      .map((e) => SurfaceState.fromJson(e as Map<String, dynamic>))
      .toList(),
  activeSurface: _surfId(json['active_surface'] as Map<String, dynamic>),
);

WorkspaceSnapshot _$WorkspaceSnapshotFromJson(Map<String, dynamic> json) =>
    WorkspaceSnapshot(
      workspace: _wsId(json['workspace'] as Map<String, dynamic>),
      name: json['name'] as String,
      layout: _layout(json['layout'] as Map<String, dynamic>),
      activePane: _paneId(json['active_pane'] as Map<String, dynamic>),
      panes: (json['panes'] as List<dynamic>)
          .map((e) => PaneSnapshot.fromJson(e as Map<String, dynamic>))
          .toList(),
    );

SessionSnapshot _$SessionSnapshotFromJson(Map<String, dynamic> json) =>
    SessionSnapshot(
      session: _sessId(json['session'] as Map<String, dynamic>),
      activeWorkspace: _wsId(json['active_workspace'] as Map<String, dynamic>),
      workspaces: (json['workspaces'] as List<dynamic>)
          .map((e) => WorkspaceSnapshot.fromJson(e as Map<String, dynamic>))
          .toList(),
    );
