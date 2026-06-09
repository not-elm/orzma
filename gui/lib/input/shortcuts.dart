import 'package:flutter/services.dart';
import '../proto/ids.dart';
import '../proto/layout.dart';
import '../proto/messages.dart';

/// The mirror state a shortcut acts on: the active workspace/pane plus the
/// ordering context needed for prev/next surface and workspace navigation.
class ShortcutContext {
  /// The currently active workspace.
  final WorkspaceId workspace;

  /// The currently active pane.
  final PaneId activePane;

  /// Ordered list of surfaces in the active pane (for prev/next navigation).
  final List<SurfaceId> surfacesInActivePane;

  /// The currently active surface in the active pane.
  final SurfaceId? activeSurface;

  /// Ordered list of workspaces in the session (for prev/next navigation).
  final List<WorkspaceId> workspaceOrder;

  /// Creates a shortcut context.
  const ShortcutContext({
    required this.workspace,
    required this.activePane,
    this.surfacesInActivePane = const [],
    this.activeSurface,
    this.workspaceOrder = const [],
  });
}

/// Resolves a key chord (logical key + pressed modifiers) to the `ClientMessage`
/// it should send, or null if the chord is unmapped. The Bevy default keymap:
/// Cmd+I/O split, Cmd+H/J/K/L navigate, Cmd+Shift+D close, Cmd+B/N swap,
/// Cmd+T new tab, Cmd+[/] focus surface, Cmd+R new workspace, Cmd+Shift+[/]
/// switch workspace.
ClientMessage? resolveShortcut(
    LogicalKeyboardKey key, Set<LogicalKeyboardKey> mods, ShortcutContext ctx) {
  final meta = _has(mods, LogicalKeyboardKey.meta, LogicalKeyboardKey.metaLeft,
      LogicalKeyboardKey.metaRight);
  final shift = _has(mods, LogicalKeyboardKey.shift,
      LogicalKeyboardKey.shiftLeft, LogicalKeyboardKey.shiftRight);
  if (!meta) return null;
  final pane = ctx.activePane;

  if (shift) {
    if (key == LogicalKeyboardKey.keyD) return ClientMessage.close(pane);
    if (key == LogicalKeyboardKey.bracketLeft) {
      final w = _neighbor(ctx.workspaceOrder, ctx.workspace, -1);
      return w == null ? null : ClientMessage.selectWorkspace(w);
    }
    if (key == LogicalKeyboardKey.bracketRight) {
      final w = _neighbor(ctx.workspaceOrder, ctx.workspace, 1);
      return w == null ? null : ClientMessage.selectWorkspace(w);
    }
    return null;
  }

  if (key == LogicalKeyboardKey.keyI) {
    return ClientMessage.split(pane: pane, orientation: SplitOrientation.vertical);
  }
  if (key == LogicalKeyboardKey.keyO) {
    return ClientMessage.split(pane: pane, orientation: SplitOrientation.horizontal);
  }
  if (key == LogicalKeyboardKey.keyH) return ClientMessage.navigate(pane, PaneDirection.left);
  if (key == LogicalKeyboardKey.keyJ) return ClientMessage.navigate(pane, PaneDirection.down);
  if (key == LogicalKeyboardKey.keyK) return ClientMessage.navigate(pane, PaneDirection.up);
  if (key == LogicalKeyboardKey.keyL) return ClientMessage.navigate(pane, PaneDirection.right);
  if (key == LogicalKeyboardKey.keyB) return ClientMessage.swapPane(pane, SwapOffset.prev);
  if (key == LogicalKeyboardKey.keyN) return ClientMessage.swapPane(pane, SwapOffset.next);
  if (key == LogicalKeyboardKey.keyT) return ClientMessage.spawnSurface(pane);
  if (key == LogicalKeyboardKey.keyR) return ClientMessage.createWorkspace();
  if (key == LogicalKeyboardKey.bracketLeft) {
    final s = _neighbor(ctx.surfacesInActivePane, ctx.activeSurface, -1);
    return s == null ? null : ClientMessage.setActiveSurface(s);
  }
  if (key == LogicalKeyboardKey.bracketRight) {
    final s = _neighbor(ctx.surfacesInActivePane, ctx.activeSurface, 1);
    return s == null ? null : ClientMessage.setActiveSurface(s);
  }
  return null;
}

bool _has(Set<LogicalKeyboardKey> mods, LogicalKeyboardKey synonym,
        LogicalKeyboardKey left, LogicalKeyboardKey right) =>
    mods.contains(synonym) || mods.contains(left) || mods.contains(right);

// NOTE: Dart's `%` on a negative left operand returns a non-negative result for
// a positive modulus (e.g. `(-1) % 2 == 1`), so delta=-1 wraps correctly.
T? _neighbor<T>(List<T> items, T? current, int delta) {
  if (items.length < 2 || current == null) return null;
  final i = items.indexOf(current);
  if (i < 0) return null;
  return items[(i + delta) % items.length];
}
