use super::constants::BONDING_CURVE_EPSILON;

// --- Bonding Curve Logic ---

// Price function P(s)
pub fn get_price(supply: f64) -> f64 {
    if supply > BONDING_CURVE_EPSILON { // s > 0
        1.0 + supply.sqrt()
    } else if supply < -BONDING_CURVE_EPSILON { // s < 0
        let t = supply.abs();
        1.0 / (1.0 + t.sqrt())
    } else { // s == 0
        1.0 // Limit from s>0
    }
}

// Integral of P(s) from 0 to s, for s > 0
// Int(1 + sqrt(x) dx) = x + (2/3)x^(3/2)
fn integral_pos(s: f64) -> f64 {
    if s <= BONDING_CURVE_EPSILON { // Treat s<=0 as 0
        0.0
    } else {
        s + (2.0 / 3.0) * s.powf(1.5)
    }
}

// Integral of P(s) from s to 0, for s < 0. Result is >= 0.
// See original code for derivation
fn integral_neg_to_zero(s: f64) -> f64 {
    if s >= -BONDING_CURVE_EPSILON { // Treat s>=0 as 0
        0.0
    } else {
        let t = s.abs(); // t = |s|
        2.0 * t.sqrt() - 2.0 * (1.0 + t.sqrt()).ln()
    }
}

// Calculate the base cost (definite integral) using the smooth curve P(s)
// from supply s1 to s2.
pub fn calculate_smooth_cost(s1: f64, s2: f64) -> f64 {
    if s1.is_nan() || s1.is_infinite() || s2.is_nan() || s2.is_infinite() {
        return f64::NAN;
    }

    let integral_at_s2 = if s2 > BONDING_CURVE_EPSILON {
        integral_pos(s2)
    } else if s2 < -BONDING_CURVE_EPSILON {
        -integral_neg_to_zero(s2)
    } else {
        0.0
    };

    let integral_at_s1 = if s1 > BONDING_CURVE_EPSILON {
        integral_pos(s1)
    } else if s1 < -BONDING_CURVE_EPSILON {
        -integral_neg_to_zero(s1)
    } else {
        0.0
    };

    integral_at_s2 - integral_at_s1
} 