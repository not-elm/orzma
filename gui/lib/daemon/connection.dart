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
    _socket.add(frameMessage(body));
  }

  /// Closes the connection.
  Future<void> close() => _socket.close();

  void _onData(Uint8List chunk) {
    for (final body in _decoder.addChunk(chunk)) {
      if (_peekFirstKey(body) == 'Frame') {
        _controller.add(const FrameMessage());
        continue;
      }
      try {
        final decoded = jsonDecode(utf8.decode(body));
        if (decoded is! Map<String, dynamic>) {
          _controller.addError(FormatException('expected a JSON object message'));
          continue;
        }
        _controller.add(ServerMessage.fromJson(decoded));
      } on FormatException catch (e) {
        _controller.addError(e);
      } on TypeError catch (e) {
        _controller.addError(StateError('malformed server message: $e'));
      }
    }
  }
}

final RegExp _firstKeyPattern = RegExp(r'^\{\s*"([A-Za-z]+)"');

String? _peekFirstKey(Uint8List body) {
  if (body.isEmpty || body[0] != 0x7b /* { */) return null;
  final end = body.length < 64 ? body.length : 64;
  // ASCII-only scan: the tag and JSON structural bytes are all ASCII, so a
  // latin1 decode of the prefix is allocation-light and never throws.
  final s = String.fromCharCodes(body, 0, end);
  return _firstKeyPattern.firstMatch(s)?.group(1);
}
