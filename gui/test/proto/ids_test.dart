import 'package:flutter_test/flutter_test.dart';
import 'package:gui/proto/ids.dart';

void main() {
  test('equality + hashCode by (idx, version)', () {
    final a = SurfaceId.fromJson({'idx': 3, 'version': 5});
    final b = SurfaceId.fromJson({'idx': 3, 'version': 5});
    final c = SurfaceId.fromJson({'idx': 3, 'version': 7});
    expect(a, b);
    expect(a.hashCode, b.hashCode);
    expect(a == c, isFalse);
    expect({a, b, c}.length, 2);
  });

  test('round-trips to the {idx, version} wire shape', () {
    final j = {'idx': 9, 'version': 1};
    expect(PaneId.fromJson(j).toJson(), j);
  });
}
