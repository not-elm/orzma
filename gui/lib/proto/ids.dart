import 'package:meta/meta.dart';

/// An opaque slotmap key (`{idx, version}`). The GUI never mints ids — it only
/// echoes server-issued ones back — so value equality is all that is needed.
@immutable
abstract class SlotId {
  final int idx;
  final int version;
  const SlotId(this.idx, this.version);

  Map<String, dynamic> toJson() => {'idx': idx, 'version': version};

  @override
  bool operator ==(Object other) =>
      other is SlotId &&
      runtimeType == other.runtimeType &&
      idx == other.idx &&
      version == other.version;

  @override
  int get hashCode => Object.hash(runtimeType, idx, version);

  @override
  String toString() => '$runtimeType($idx,$version)';
}

/// A session id.
class SessionId extends SlotId {
  const SessionId(super.idx, super.version);
  factory SessionId.fromJson(Map<String, dynamic> j) =>
      SessionId(j['idx'] as int, j['version'] as int);
}

/// A workspace id.
class WorkspaceId extends SlotId {
  const WorkspaceId(super.idx, super.version);
  factory WorkspaceId.fromJson(Map<String, dynamic> j) =>
      WorkspaceId(j['idx'] as int, j['version'] as int);
}

/// A pane id.
class PaneId extends SlotId {
  const PaneId(super.idx, super.version);
  factory PaneId.fromJson(Map<String, dynamic> j) =>
      PaneId(j['idx'] as int, j['version'] as int);
}

/// A split id.
class SplitId extends SlotId {
  const SplitId(super.idx, super.version);
  factory SplitId.fromJson(Map<String, dynamic> j) =>
      SplitId(j['idx'] as int, j['version'] as int);
}

/// A surface id.
class SurfaceId extends SlotId {
  const SurfaceId(super.idx, super.version);
  factory SurfaceId.fromJson(Map<String, dynamic> j) =>
      SurfaceId(j['idx'] as int, j['version'] as int);
}
