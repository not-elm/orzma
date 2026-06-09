import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:gui/proto/snapshot.dart';
import 'package:gui/proto/messages.dart';
import 'package:gui/proto/mux_event.dart';
import 'package:gui/proto/ids.dart';
import 'package:gui/session/session.dart';

SessionSnapshot welcomeFixture() {
  final raw = jsonDecode(File('test/fixtures/welcome.json').readAsStringSync());
  return SessionSnapshot.fromJson(raw['Welcome']['snapshot'] as Map<String, dynamic>);
}

void main() {
  test('folds Welcome + Events and drops Frames', () async {
    final controller = StreamController<ServerMessage>();
    final session = Session(incoming: controller.stream, send: (_) {});
    controller.add(WelcomeMessage(welcomeFixture()));
    await Future<void>.delayed(Duration.zero);
    expect(session.state, isNotNull);

    const other = WorkspaceId(42, 1);
    controller.add(EventsMessage([const WorkspaceSelected(other)]));
    await Future<void>.delayed(Duration.zero);
    expect(session.state!.activeWorkspace, other);

    controller.add(const FrameMessage()); // must not throw / change panes
    await Future<void>.delayed(Duration.zero);
    expect(session.state!.workspaces.first.panes.length, 2);

    session.dispose();
    await controller.close();
  });

  test('dispatchShortcut sends a Split for Cmd+I', () async {
    final controller = StreamController<ServerMessage>();
    final sent = <ClientMessage>[];
    final session = Session(incoming: controller.stream, send: sent.add);
    controller.add(WelcomeMessage(welcomeFixture()));
    await Future<void>.delayed(Duration.zero);

    final handled = session.dispatchShortcut(
        LogicalKeyboardKey.keyI, {LogicalKeyboardKey.meta});
    expect(handled, isTrue);
    expect(sent.single.toJson().keys.first, 'Split');

    final ignored = session.dispatchShortcut(LogicalKeyboardKey.keyZ, {});
    expect(ignored, isFalse);

    session.dispose();
    await controller.close();
  });
}
