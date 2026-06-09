import 'dart:convert';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:gui/daemon/framing.dart';

void main() {
  test('frame prefixes a u32 BE length', () {
    final body = utf8.encode('{"Health":null}');
    final framed = frameMessage(Uint8List.fromList(body));
    final view = ByteData.sublistView(framed);
    expect(view.getUint32(0, Endian.big), body.length);
    expect(framed.sublist(4), body);
  });

  test('FrameDecoder yields complete bodies across chunk splits', () {
    final a = frameMessage(Uint8List.fromList(utf8.encode('AB')));
    final b = frameMessage(Uint8List.fromList(utf8.encode('CDE')));
    final all = Uint8List.fromList([...a, ...b]);
    final dec = FrameDecoder();
    final out = <Uint8List>[];
    // Feed one byte at a time to prove reassembly.
    for (final byte in all) {
      out.addAll(dec.addChunk(Uint8List.fromList([byte])));
    }
    expect(out.map(utf8.decode).toList(), ['AB', 'CDE']);
  });
}
