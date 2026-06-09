import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import '../daemon/connection.dart';
import '../input/shortcuts.dart';
import '../proto/ids.dart';
import '../proto/messages.dart';
import '../proto/vt_event.dart';
import 'mirror.dart';

/// Owns the daemon connection's message stream: folds Welcome + Events into a
/// [ClientMirror], drops terminal Frames, applies cwd updates, and turns key
/// chords into outgoing layout commands. Notifies listeners on every state change.
class Session extends ChangeNotifier {
  final void Function(ClientMessage) _send;
  ClientMirror? _mirror;
  StreamSubscription<ServerMessage>? _sub;

  /// Builds a session over an explicit message stream + send sink (testable).
  Session({required Stream<ServerMessage> incoming, required this._send}) {
    _sub = incoming.listen(_onMessage);
  }

  /// Builds a session bound to a live [DaemonConnection].
  factory Session.fromConnection(DaemonConnection conn) =>
      Session(incoming: conn.messages, send: conn.send);

  /// The current reconstructed session state, or null before the first Welcome.
  SessionState? get state => _mirror?.state;

  void _onMessage(ServerMessage m) {
    switch (m) {
      case WelcomeMessage(:final snapshot):
        _mirror = ClientMirror.fromSnapshot(snapshot);
        notifyListeners();
      case EventsMessage(:final events):
        _mirror?.applyEvents(events);
        notifyListeners();
      case SurfaceEventMessage(:final surface, :final event):
        _applySurfaceEvent(surface, event);
        notifyListeners();
      case FrameMessage():
        break; // terminal content — discarded by the layout client
      case ErrorMessage():
        break; // v1: surfaced via no UI yet
      case UnknownServerMessage():
        break;
    }
  }

  void _applySurfaceEvent(SurfaceId surface, VtEvent event) {
    final mirror = _mirror;
    if (mirror == null) return;
    if (event is CurrentDir) {
      for (final ws in mirror.state.workspaces) {
        for (final p in ws.panes) {
          for (final s in p.surfaces) {
            if (s.surface == surface) s.cwd = event.path;
          }
        }
      }
    }
  }

  /// Resolves a key chord against the active pane/workspace and sends the
  /// resulting command. Returns true if a command was sent.
  bool dispatchShortcut(LogicalKeyboardKey key, Set<LogicalKeyboardKey> mods) {
    final st = state;
    if (st == null) return false;
    final ws = _activeWorkspace(st);
    if (ws == null) return false;
    final activePane = _paneById(ws, ws.activePane);
    final ctx = ShortcutContext(
      workspace: ws.workspace,
      activePane: ws.activePane,
      surfacesInActivePane:
          activePane?.surfaces.map((s) => s.surface).toList() ?? const [],
      activeSurface: activePane?.activeSurface,
      workspaceOrder: st.workspaces.map((w) => w.workspace).toList(),
    );
    final msg = resolveShortcut(key, mods, ctx);
    if (msg == null) return false;
    _send(msg);
    return true;
  }

  MutableWorkspace? _activeWorkspace(SessionState st) {
    for (final w in st.workspaces) {
      if (w.workspace == st.activeWorkspace) return w;
    }
    return st.workspaces.isEmpty ? null : st.workspaces.first;
  }

  MutablePane? _paneById(MutableWorkspace ws, PaneId id) {
    for (final p in ws.panes) {
      if (p.pane == id) return p;
    }
    return null;
  }

  @override
  void dispose() {
    _sub?.cancel();
    super.dispose();
  }
}
