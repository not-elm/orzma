import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import '../proto/messages.dart';
import 'framing.dart';

/// A framed connection to the ozmux daemon over a Unix-domain socket. Decodes
/// incoming `[u32 BE len][json]` frames into `ServerMessage`s; terminal `Frame`
/// messages are detected by tag and NOT fully decoded (drained cheaply so the
/// client never lags the daemon's broadcast). `messages` is a single-
/// subscription stream that buffers until listened, so the cold-attach `Welcome`
/// is not lost between connect and the first `listen`.
class DaemonConnection {
  final Socket _socket;
  final FrameDecoder _decoder = FrameDecoder();
  final StreamController<ServerMessage> _controller =
      StreamController<ServerMessage>();

  DaemonConnection._(this._socket) {
    _socket.listen(_onData,
        onDone: _controller.close,
        onError: _controller.addError,
        cancelOnError: false);
  }

  /// Opens a framed connection to the daemon at `path`.
  static Future<DaemonConnection> connect(String path) async {
    final s = await Socket.connect(
        InternetAddress(path, type: InternetAddressType.unix), 0);
    return DaemonConnection._(s);
  }

  /// Incoming messages (single subscription; buffered until listened).
  Stream<ServerMessage> get messages => _controller.stream;

  /// Sends a command to the daemon.
  void send(ClientMessage msg) {
    final body = utf8.encode(jsonEncode(msg.toJson()));
    _socket.add(frameMessage(Uint8List.fromList(body)));
  }

  /// Closes the connection.
  Future<void> close() => _socket.close();

  void _onData(Uint8List chunk) {
    for (final body in _decoder.addChunk(chunk)) {
      if (_peekFirstKey(body) == 'Frame') {
        _controller.add(const FrameMessage());
        continue;
      }
      _controller.add(ServerMessage.fromJson(
          jsonDecode(utf8.decode(body)) as Map<String, dynamic>));
    }
  }
}

String? _peekFirstKey(Uint8List body) {
  if (body.isEmpty || body[0] != 0x7b /* { */) return null;
  final end = body.length < 64 ? body.length : 64;
  final s = utf8.decode(body.sublist(0, end), allowMalformed: true);
  final m = RegExp(r'^\{\s*"([A-Za-z]+)"').firstMatch(s);
  return m?.group(1);
}
