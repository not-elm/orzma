/// Converts a split ratio (first-child fraction in [0,1]) to a pair of positive
/// integer flex weights for Flutter `Expanded(flex:)`. Clamps so neither child
/// collapses to zero flex (Flutter requires flex > 0); a non-finite ratio
/// rescues to 0.5.
(int, int) flexWeights(double ratio) {
  final r = ratio.isFinite ? ratio.clamp(0.0, 1.0) : 0.5;
  final first = (r * 1000).round().clamp(1, 999);
  return (first, 1000 - first);
}
