import 'ids.dart';
import 'layout.dart';
import 'snapshot.dart';
import 'mux_event.dart';
import 'vt_event.dart';

/// Cardinal direction for a `Navigate` command.
enum PaneDirection { left, right, up, down }

/// Neighbor selector for a `SwapPane` command.
enum SwapOffset { prev, next }

String _dir(PaneDirection d) => switch (d) {
      PaneDirection.left => 'Left',
      PaneDirection.right => 'Right',
      PaneDirection.up => 'Up',
      PaneDirection.down => 'Down',
    };

/// A command sent from the GUI to the daemon (serde externally-tagged).
class ClientMessage {
  final String _tag;
  final Map<String, dynamic> _payload;
  const ClientMessage._(this._tag, this._payload);

  /// The externally-tagged wire map `{Tag: {fields}}`.
  Map<String, dynamic> toJson() => {_tag: _payload};

  /// Split `pane` along `orientation`, inserting a new Terminal surface after it.
  factory ClientMessage.split({
    required PaneId pane,
    required SplitOrientation orientation,
  }) =>
      ClientMessage._('Split', {
        'pane': pane.toJson(),
        'orientation':
            orientation == SplitOrientation.vertical ? 'Vertical' : 'Horizontal',
        'side': 'After',
        'kind': 'Terminal',
        'cwd': null,
      });

  /// Move focus from `pane` in `direction`.
  factory ClientMessage.navigate(PaneId pane, PaneDirection direction) =>
      ClientMessage._('Navigate', {'pane': pane.toJson(), 'direction': _dir(direction)});

  /// Close `pane`.
  factory ClientMessage.close(PaneId pane) =>
      ClientMessage._('Close', {'pane': pane.toJson()});

  /// Make `pane` the active pane in `workspace`.
  factory ClientMessage.setActivePane(WorkspaceId workspace, PaneId pane) =>
      ClientMessage._('SetActivePane', {'workspace': workspace.toJson(), 'pane': pane.toJson()});

  /// Make `surface` the active surface in its pane.
  factory ClientMessage.setActiveSurface(SurfaceId surface) =>
      ClientMessage._('SetActiveSurface', {'surface': surface.toJson()});

  /// Swap `pane` with its prev/next neighbor in the layout.
  factory ClientMessage.swapPane(PaneId pane, SwapOffset offset) =>
      ClientMessage._('SwapPane', {
        'pane': pane.toJson(),
        'offset': offset == SwapOffset.next ? 'Next' : 'Prev',
      });

  /// Spawn a new Terminal surface (tab) in `pane`.
  factory ClientMessage.spawnSurface(PaneId pane) =>
      ClientMessage._('SpawnSurface', {'pane': pane.toJson(), 'kind': 'Terminal', 'cwd': null});

  /// Create a workspace (daemon auto-names when `name` is null).
  factory ClientMessage.createWorkspace({String? name}) =>
      ClientMessage._('CreateWorkspace', {'name': name});

  /// Switch the active workspace.
  factory ClientMessage.selectWorkspace(WorkspaceId workspace) =>
      ClientMessage._('SelectWorkspace', {'workspace': workspace.toJson()});
}

/// A message received from the daemon.
sealed class ServerMessage {
  const ServerMessage();
  static ServerMessage fromJson(Map<String, dynamic> j) {
    final key = j.keys.first;
    final v = j[key];
    switch (key) {
      case 'Welcome':
        return WelcomeMessage(
            SessionSnapshot.fromJson(v['snapshot'] as Map<String, dynamic>));
      case 'Events':
        return EventsMessage(
            (v as List).map((e) => MuxEvent.fromJson(e as Object)).toList());
      case 'SurfaceEvent':
        return SurfaceEventMessage(
            SurfaceId.fromJson(v['surface'] as Map<String, dynamic>),
            VtEvent.fromJson(v['event'] as Object));
      case 'Frame':
        return const FrameMessage();
      case 'Error':
        return ErrorMessage(v['message'] as String);
      default:
        return UnknownServerMessage(key);
    }
  }
}

/// The cold-attach snapshot.
class WelcomeMessage extends ServerMessage {
  final SessionSnapshot snapshot;
  const WelcomeMessage(this.snapshot);
}

/// A batch of mux events to fold into the mirror.
class EventsMessage extends ServerMessage {
  final List<MuxEvent> events;
  const EventsMessage(this.events);
}

/// A per-surface VT control event (title/cwd/…).
class SurfaceEventMessage extends ServerMessage {
  final SurfaceId surface;
  final VtEvent event;
  const SurfaceEventMessage(this.surface, this.event);
}

/// A terminal frame — discarded by the layout client (marker only, no body decode).
class FrameMessage extends ServerMessage {
  const FrameMessage();
}

/// A rejected command.
class ErrorMessage extends ServerMessage {
  final String message;
  const ErrorMessage(this.message);
}

/// An unrecognized server message tag (forward-compat).
class UnknownServerMessage extends ServerMessage {
  final String tag;
  const UnknownServerMessage(this.tag);
}
