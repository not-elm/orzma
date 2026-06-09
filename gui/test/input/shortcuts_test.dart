import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:gui/input/shortcuts.dart';
import 'package:gui/proto/ids.dart';

void main() {
  const ctx = ShortcutContext(
      workspace: WorkspaceId(1, 1), activePane: PaneId(2, 1));

  test('Cmd+I → vertical split of active pane', () {
    final j = resolveShortcut(
            LogicalKeyboardKey.keyI, {LogicalKeyboardKey.meta}, ctx)!
        .toJson();
    expect(j['Split']['orientation'], 'Vertical');
    expect(j['Split']['pane'], {'idx': 2, 'version': 1});
  });

  test('Cmd+H → navigate left; Cmd+O → horizontal split', () {
    expect(
        resolveShortcut(LogicalKeyboardKey.keyH, {LogicalKeyboardKey.meta}, ctx)!
            .toJson()['Navigate']['direction'],
        'Left');
    expect(
        resolveShortcut(LogicalKeyboardKey.keyO, {LogicalKeyboardKey.meta}, ctx)!
            .toJson()['Split']['orientation'],
        'Horizontal');
  });

  test('Cmd+Shift+D → close active pane', () {
    final j = resolveShortcut(LogicalKeyboardKey.keyD,
            {LogicalKeyboardKey.meta, LogicalKeyboardKey.shift}, ctx)!
        .toJson();
    expect(j['Close']['pane'], {'idx': 2, 'version': 1});
  });

  test('Cmd+] → next surface using context order (wraps)', () {
    const c = ShortcutContext(
      workspace: WorkspaceId(1, 1),
      activePane: PaneId(2, 1),
      surfacesInActivePane: [SurfaceId(10, 1), SurfaceId(11, 1)],
      activeSurface: SurfaceId(11, 1),
    );
    final j = resolveShortcut(
            LogicalKeyboardKey.bracketRight, {LogicalKeyboardKey.meta}, c)!
        .toJson();
    expect(j['SetActiveSurface']['surface'], {'idx': 10, 'version': 1}); // wrapped
  });

  test('Cmd+Shift+] → next workspace; physical metaLeft also works', () {
    const c = ShortcutContext(
      workspace: WorkspaceId(1, 1),
      activePane: PaneId(2, 1),
      workspaceOrder: [WorkspaceId(1, 1), WorkspaceId(5, 1)],
    );
    final j = resolveShortcut(LogicalKeyboardKey.bracketRight,
            {LogicalKeyboardKey.metaLeft, LogicalKeyboardKey.shift}, c)!
        .toJson();
    expect(j['SelectWorkspace']['workspace'], {'idx': 5, 'version': 1});
  });

  test('unmapped chord → null; no-modifier letter → null', () {
    expect(resolveShortcut(LogicalKeyboardKey.keyZ, {LogicalKeyboardKey.meta}, ctx), isNull);
    expect(resolveShortcut(LogicalKeyboardKey.keyI, {}, ctx), isNull);
  });
}
