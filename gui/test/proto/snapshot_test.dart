import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:gui/proto/snapshot.dart';

void main() {
  test('parses the real Welcome snapshot fixture', () {
    final raw = jsonDecode(File('test/fixtures/welcome.json').readAsStringSync());
    final snap = SessionSnapshot.fromJson(
        raw['Welcome']['snapshot'] as Map<String, dynamic>);
    expect(snap.workspaces, isNotEmpty);
    final ws = snap.workspaces.first;
    expect(ws.panes.length, 2); // fixture split once
    expect(ws.activePane, isNotNull);
    expect(ws.panes.first.surfaces.first.kind, isNotNull);
  });
}
