import 'dart:math';

// --- Bonding Curve Calculation Helpers (Dart mirror of Rust logic) ---

const double _BONDING_CURVE_EPSILON = 1e-9;
const double EPSILON = 1e-9; // General purpose epsilon (exported)

// Integral of P(s) from 0 to s, for s > 0
// Int(1 + sqrt(x) dx) = x + (2/3)x^(3/2)
double _integralPos(double s) {
  if (s <= _BONDING_CURVE_EPSILON) { // Treat s<=0 as 0
    return 0.0;
  } else {
    return s + (2.0 / 3.0) * pow(s, 1.5);
  }
}

// Integral of P(s) from s to 0, for s < 0. Result is >= 0.
// 2*sqrt(|s|) - 2*ln(1+sqrt(|s|))
double _integralNegToZero(double s) {
  if (s >= -_BONDING_CURVE_EPSILON) { // Treat s>=0 as 0
    return 0.0;
  } else {
    final t = s.abs(); // t = |s|
    // Add small epsilon to log argument to prevent log(1) = 0 if t is very small negative
    // However, the check `s >= -_BONDING_CURVE_EPSILON` should handle s close to 0.
    // If t is exactly 0, log(1) is fine. The main concern is numerical stability.
    // Let's keep the original logic unless issues arise.
    return 2.0 * sqrt(t) - 2.0 * log(1.0 + sqrt(t));
  }
}

// Calculate the cost (definite integral) of changing supply from s1 to s2
// Cost = Integral[s1, s2] P(x) dx
//      = Integral[0, s2] P(x) dx - Integral[0, s1] P(x) dx
double calculateBondingCurveCost(double s1, double s2) {
  if (s1.isNaN || s1.isInfinite || s2.isNaN || s2.isInfinite) {
    print("Warning: calculateBondingCurveCost received NaN or Infinite input (s1: $s1, s2: $s2)");
    return double.nan;
  }

  final integralAtS2 = (s2 > _BONDING_CURVE_EPSILON)
      ? _integralPos(s2)
      : (s2 < -_BONDING_CURVE_EPSILON ? -_integralNegToZero(s2) : 0.0);

  final integralAtS1 = (s1 > _BONDING_CURVE_EPSILON)
      ? _integralPos(s1)
      : (s1 < -_BONDING_CURVE_EPSILON ? -_integralNegToZero(s1) : 0.0);

  // Check for potential NaN results from individual integrals, though less likely with input checks
  if (integralAtS1.isNaN || integralAtS2.isNaN) {
       print("Warning: calculateBondingCurveCost produced NaN integral (integralAtS1: $integralAtS1, integralAtS2: $integralAtS2)");
       return double.nan;
  }

  return integralAtS2 - integralAtS1;
} 