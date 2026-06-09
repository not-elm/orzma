import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:gui/ui/workspace_view.dart';
import 'package:gui/ui/status_strip.dart';
import 'package:gui/session/mirror.dart';
import 'package:gui/proto/layout.dart';
import 'package:gui/proto/ids.dart';

void main() {
  testWidgets('renders one placeholder per pane + active accent border', (tester) async {
    const p1 = PaneId(2, 1), p2 = PaneId(3, 1);
    const s1 = SurfaceId(9, 1), s2 = SurfaceId(10, 1);
    const layout = LayoutSplit(
      id: SplitId(1, 1),
      orientation: SplitOrientation.horizontal,
      ratio: 0.5,
      first: LayoutPane(id: p1, surfaceKind: TerminalKind()),
      second: LayoutPane(id: p2, surfaceKind: TerminalKind()),
    );
    final ws = MutableWorkspace(
      workspace: const WorkspaceId(1, 1),
      name: 'ws',
      layout: layout,
      activePane: p1,
      panes: [
        MutablePane(p1, [MutableSurface(s1, const TerminalKind(), '~/a')], s1),
        MutablePane(p2, [MutableSurface(s2, const TerminalKind(), '~/b')], s2),
      ],
    );
    await tester.pumpWidget(
        MaterialApp(home: Scaffold(body: WorkspaceView(workspace: ws))));
    expect(find.text('● terminal'), findsNWidgets(2));
    expect(find.text('~/a'), findsOneWidget);
  });

  testWidgets('status strip shows connection state + workspace chips', (tester) async {
    await tester.pumpWidget(const MaterialApp(
      home: Scaffold(
        body: StatusStrip(
            state: ConnState.connected,
            workspaces: ['1 a', '2 b'],
            activeWorkspace: 0),
      ),
    ));
    expect(find.text('connected'), findsOneWidget);
    expect(find.text('1 a'), findsOneWidget);
    expect(find.text('2 b'), findsOneWidget);
  });
}
