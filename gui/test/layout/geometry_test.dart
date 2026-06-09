import 'package:flutter_test/flutter_test.dart';
import 'package:gui/layout/geometry.dart';

void main() {
  test('ratio → integer flex weights in proportion', () {
    final (a, b) = flexWeights(0.25);
    expect(a / (a + b), closeTo(0.25, 0.001));
  });
  test('degenerate ratios stay positive (Flutter requires flex > 0)', () {
    final (a0, b0) = flexWeights(0.0);
    expect(a0, greaterThan(0));
    expect(b0, greaterThan(0));
    final (a1, b1) = flexWeights(1.0);
    expect(a1, greaterThan(0));
    expect(b1, greaterThan(0));
  });
  test('non-finite ratio rescues to ~half', () {
    final (a, b) = flexWeights(double.nan);
    expect(a, closeTo(b.toDouble(), 2));
  });
}
