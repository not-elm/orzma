import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:gui/proto/mux_event.dart';
import 'package:gui/proto/ids.dart';

void main() {
  test('parses the real split Events fixture (PaneCreated + LayoutChanged)', () {
    final raw = jsonDecode(File('test/fixtures/events_split.json').readAsStringSync());
    final events = (raw['Events'] as List)
        .map((e) => MuxEvent.fromJson(e as Object))
        .toList();
    expect(events.any((e) => e is PaneCreated), isTrue);
    expect(events.any((e) => e is LayoutChanged), isTrue);
    expect(events.whereType<PaneResized>().length, 2);
  });

  test('ActivePaneChanged parses workspace + pane', () {
    final e = MuxEvent.fromJson({
      'ActivePaneChanged': {
        'workspace': {'idx': 1, 'version': 1},
        'pane': {'idx': 2, 'version': 1},
      }
    }) as ActivePaneChanged;
    expect(e.pane, const PaneId(2, 1));
  });

  test('PaneCreated parses its surface manifest', () {
    final raw = jsonDecode(File('test/fixtures/events_split.json').readAsStringSync());
    final pc = (raw['Events'] as List)
        .map((e) => MuxEvent.fromJson(e as Object))
        .whereType<PaneCreated>()
        .first;
    expect(pc.surfaces, isNotEmpty);
    expect(pc.activeSurface, isNotNull);
  });
}
