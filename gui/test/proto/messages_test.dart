import 'package:flutter_test/flutter_test.dart';
import 'package:gui/proto/messages.dart';
import 'package:gui/proto/ids.dart';
import 'package:gui/proto/layout.dart';

void main() {
  test('Close encodes externally-tagged', () {
    expect(ClientMessage.close(const PaneId(2, 1)).toJson(),
        {'Close': {'pane': {'idx': 2, 'version': 1}}});
  });

  test('Split encodes orientation/side/kind/cwd', () {
    final j = ClientMessage.split(
            pane: const PaneId(2, 1), orientation: SplitOrientation.vertical)
        .toJson();
    expect(j['Split']['orientation'], 'Vertical');
    expect(j['Split']['side'], 'After');
    expect(j['Split']['kind'], 'Terminal');
    expect(j['Split']['cwd'], isNull);
    expect(j['Split']['pane'], {'idx': 2, 'version': 1});
  });

  test('Navigate + SwapPane + workspace messages encode', () {
    expect(ClientMessage.navigate(const PaneId(1, 1), PaneDirection.left).toJson()['Navigate']['direction'], 'Left');
    expect(ClientMessage.swapPane(const PaneId(1, 1), SwapOffset.next).toJson()['SwapPane']['offset'], 'Next');
    expect(ClientMessage.createWorkspace().toJson(), {'CreateWorkspace': {'name': null}});
    expect(ClientMessage.selectWorkspace(const WorkspaceId(3, 1)).toJson()['SelectWorkspace']['workspace'], {'idx': 3, 'version': 1});
  });

  test('ServerMessage tags Frame without decoding the body', () {
    final m = ServerMessage.fromJson({'Frame': {'surface': {'idx': 1, 'version': 1}, 'frame': {'Snapshot': {'huge': true}}}});
    expect(m, isA<FrameMessage>());
  });

  test('ServerMessage parses Welcome/Events/Error', () {
    expect(ServerMessage.fromJson({'Error': {'message': 'x'}}), isA<ErrorMessage>());
    final ev = ServerMessage.fromJson({'Events': []});
    expect(ev, isA<EventsMessage>());
    expect((ev as EventsMessage).events, isEmpty);
  });
}
