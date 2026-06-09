import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:gui/proto/snapshot.dart';
import 'package:gui/proto/mux_event.dart';
import 'package:gui/proto/ids.dart';
import 'package:gui/proto/layout.dart';
import 'package:gui/session/mirror.dart';

SessionSnapshot welcomeFixture() {
  final raw = jsonDecode(File('test/fixtures/welcome.json').readAsStringSync());
  return SessionSnapshot.fromJson(raw['Welcome']['snapshot'] as Map<String, dynamic>);
}

void main() {
  test('fromSnapshot mirrors the workspace + panes', () {
    final m = ClientMirror.fromSnapshot(welcomeFixture());
    expect(m.state.workspaces.length, 1);
    expect(m.state.workspaces.first.panes.length, 2);
    expect(m.state.workspaces.first.layout, isA<LayoutSplit>());
  });

  test('WorkspaceSelected updates the active workspace', () {
    final m = ClientMirror.fromSnapshot(welcomeFixture());
    const other = WorkspaceId(42, 1);
    m.applyEvents([const WorkspaceSelected(other)]);
    expect(m.state.activeWorkspace, other);
  });

  test('PaneClosed removes the pane from its workspace', () {
    final m = ClientMirror.fromSnapshot(welcomeFixture());
    final ws = m.state.workspaces.first;
    final victim = ws.panes.last.pane;
    final before = ws.panes.length;
    m.applyEvents([PaneClosed(victim)]);
    expect(m.state.workspaces.first.panes.length, before - 1);
  });

  test('LayoutRatioChanged updates a split ratio in the tree', () {
    final m = ClientMirror.fromSnapshot(welcomeFixture());
    final split = m.state.workspaces.first.layout as LayoutSplit;
    m.applyEvents([LayoutRatioChanged(split.id, 0.3)]);
    final after = m.state.workspaces.first.layout as LayoutSplit;
    expect(after.ratio, closeTo(0.3, 1e-6));
  });

  test('SurfaceSpawned then SurfaceClosed adjust a pane surface list', () {
    final m = ClientMirror.fromSnapshot(welcomeFixture());
    final pane = m.state.workspaces.first.panes.first;
    final before = pane.surfaces.length;
    const newSurf = SurfaceId(999, 1);
    m.applyEvents([SurfaceSpawned(pane.pane, newSurf, const TerminalKind(), '~/x')]);
    expect(m.state.workspaces.first.panes.first.surfaces.length, before + 1);
    m.applyEvents([const SurfaceClosed(newSurf)]);
    expect(m.state.workspaces.first.panes.first.surfaces.length, before);
  });

  test('LayoutChanged collapsing the root prunes the dropped pane', () {
    final m = ClientMirror.fromSnapshot(welcomeFixture());
    final ws = m.state.workspaces.first;
    final split = ws.layout as LayoutSplit;
    final keep = (split.first as LayoutPane).id;
    final drop = (split.second as LayoutPane).id;
    // Replace the whole split with just the first pane → second pane must be pruned.
    m.applyEvents([
      LayoutChanged(ws.workspace, NodeSplit(split.id),
          LayoutPane(id: keep, surfaceKind: const TerminalKind())),
    ]);
    final after = m.state.workspaces.first;
    expect(after.layout, isA<LayoutPane>());
    expect(after.panes.map((p) => p.pane).contains(drop), isFalse);
    expect(after.panes.map((p) => p.pane).contains(keep), isTrue);
  });
}
