import 'dart:typed_data';

/// Maximum decoded body size, mirroring the daemon's `MAX_MESSAGE_BYTES` (8 MiB).
/// Caps the attacker/corruption-controlled `_need` allocation on a torn prefix.
const int maxFrameBytes = 8 * 1024 * 1024;

/// Prefixes `body` with a big-endian u32 length, matching the daemon's
/// `[u32 BE len][json]` wire framing.
Uint8List frameMessage(Uint8List body) {
  final out = Uint8List(4 + body.length);
  ByteData.sublistView(out).setUint32(0, body.length, Endian.big);
  out.setRange(4, out.length, body);
  return out;
}

/// Reassembles length-prefixed message bodies from arbitrary byte chunks.
class FrameDecoder {
  final BytesBuilder _buf = BytesBuilder(copy: false);
  int? _need;

  /// Feeds a socket chunk; returns zero or more complete message bodies.
  List<Uint8List> addChunk(Uint8List chunk) {
    _buf.add(chunk);
    final out = <Uint8List>[];
    var bytes = _buf.toBytes();
    var offset = 0;
    while (true) {
      if (_need == null) {
        if (bytes.length - offset < 4) break;
        _need = ByteData.sublistView(bytes, offset, offset + 4)
            .getUint32(0, Endian.big);
        offset += 4;
        if (_need! > maxFrameBytes) {
          throw FormatException('frame length $_need exceeds $maxFrameBytes');
        }
      }
      if (bytes.length - offset < _need!) break;
      out.add(Uint8List.sublistView(bytes, offset, offset + _need!));
      offset += _need!;
      _need = null;
    }
    final remaining = Uint8List.sublistView(bytes, offset);
    _buf.clear();
    _buf.add(remaining);
    return out;
  }
}
