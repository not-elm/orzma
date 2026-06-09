import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:gui/proto/layout.dart';
import 'package:gui/proto/ids.dart';

void main() {
  test('SurfaceKind unit + data variants', () {
    expect(SurfaceKind.fromJson('Terminal'), isA<TerminalKind>());
    final ext = SurfaceKind.fromJson({'Extension': {'entry': 'a.html'}});
    expect((ext as ExtensionKind).entry, 'a.html');
  });

  test('LayoutNode parses a Split with two Pane children', () {
    final node = LayoutNode.fromJson({
      'Split': {
        'id': {'idx': 1, 'version': 1},
        'orientation': 'Horizontal',
        'ratio': 0.5,
        'first': {
          'Pane': {
            'id': {'idx': 2, 'version': 1},
            'surface_kind': 'Terminal',
            'cols': 80,
            'rows': 24,
          }
        },
        'second': {
          'Pane': {
            'id': {'idx': 3, 'version': 1},
            'surface_kind': 'Terminal',
            'cols': 80,
            'rows': 24,
          }
        },
      }
    });
    final split = node as LayoutSplit;
    expect(split.orientation, SplitOrientation.horizontal);
    expect(split.ratio, 0.5);
    expect((split.first as LayoutPane).id, const PaneId(2, 1));
  });

  test('the real Welcome fixture parses end to end', () {
    final raw = jsonDecode(File('test/fixtures/welcome.json').readAsStringSync());
    final layout = (raw['Welcome']['snapshot']['workspaces'][0]['layout']);
    final node = LayoutNode.fromJson(layout);
    expect(node, isA<LayoutSplit>()); // the fixture split once
  });
}
