import 'package:flutter_test/flutter_test.dart';
import 'package:gui/daemon/socket_path.dart';

void main() {
  test('matches the ozmux-<uid>/default.sock convention', () {
    final p = ozmuxSocketPath();
    expect(p.endsWith('default.sock'), isTrue);
    expect(p.contains('ozmux-'), isTrue);
    expect(p.length, lessThanOrEqualTo(103)); // sun_path holds NUL → ≤103
  });
}
