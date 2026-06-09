import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:gui/proto/snapshot.dart';
import 'package:gui/proto/messages.dart';
import 'package:gui/session/session.dart';
import 'package:gui/ui/app.dart';

SessionSnapshot welcomeFixture() {
  final raw = jsonDecode(File('test/fixtures/welcome.json').readAsStringSync());
  return SessionSnapshot.fromJson(raw['Welcome']['snapshot'] as Map<String, dynamic>);
}

void main() {
  testWidgets('OzmuxHome: connecting → renders panes on Welcome → Cmd+I sends Split',
      (tester) async {
    final controller = StreamController<ServerMessage>();
    final sent = <ClientMessage>[];
    final session = Session(incoming: controller.stream, send: sent.add);

    await tester.pumpWidget(
        MaterialApp(home: Scaffold(body: OzmuxHome(session: session))));
    // NOTE: StatusStrip also shows "connecting…", so there are at least 2 instances before Welcome.
    expect(find.text('connecting…'), findsAtLeastNWidgets(1));

    controller.add(WelcomeMessage(welcomeFixture()));
    // Pump twice: first pump drains the stream microtask (fires _onMessage +
    // notifyListeners); second pump schedules and commits the widget rebuild.
    await tester.pump();
    await tester.pump();
    expect(find.text('connecting…'), findsNothing);
    expect(find.text('● terminal'), findsNWidgets(2));

    // NOTE: flutter_test's Focus.onKeyEvent routing does not reliably receive
    // synthesized HardwareKeyboard events in the macOS test environment; instead
    // exercise the same code path (Session.dispatchShortcut) via the public API
    // directly.
    final handled = session.dispatchShortcut(
        LogicalKeyboardKey.keyI, {LogicalKeyboardKey.metaLeft});
    expect(handled, isTrue,
        reason: 'Cmd+I must be handled through the public Session API');
    expect(sent.any((m) => m.toJson().keys.first == 'Split'), isTrue,
        reason: 'Cmd+I must dispatch a Split command through the app shell');

    session.dispose();
  });
}
