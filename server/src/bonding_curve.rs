//! Bonding curve calculations.

/// Calculates the price based on the current supply `x`.
///
/// The bonding curve formula is:
/// - If x > 0:  price = 1 + sqrt(x)
/// - If x <= 0: price = 1 / (1 + sqrt(abs(x)))
///
/// The starting price (x=0) is 1.0.
///
/// # Arguments
///
/// * `x` - The current supply (can be positive or negative).
///
/// # Returns
///
/// The calculated price as f64.
pub fn get_price(x: f64) -> f64 {
    if x > 0.0 {
        1.0 + x.sqrt()
    } else {
        // Use abs(x) for the square root calculation when x is negative
        1.0 / (1.0 + x.abs().sqrt())
    }
}

/// Calculates the integral of the price function.
/// This is needed to determine the cost/proceeds of a trade.
/// Integral(p(x) dx)
fn integral_price(x: f64) -> f64 {
    if x > 0.0 {
        // Integral of (1 + sqrt(x)) dx = x + (2/3) * x^(3/2)
        x + (2.0 / 3.0) * x.powf(1.5)
    } else if x < 0.0 {
        // Integral of (1 / (1 + sqrt(|x|))) dx
        // Let u = sqrt(-x). Integral = -2 * (u - ln(1 + u))
        let u = (-x).sqrt();
        -2.0 * (u - (1.0 + u).ln())
    } else { // x == 0.0
        0.0 // Integral from 0 to 0 is 0
    }
}

/// Calculates the cost to buy `amount` shares when current supply is `supply`,
/// or the proceeds from selling `amount` shares (`amount` will be negative).
///
/// Cost = Integral(supply + amount) - Integral(supply)
/// Proceeds = Integral(supply) - Integral(supply + amount) [where amount < 0]
/// Which simplifies to the same formula regardless of amount sign.
///
/// Returns a positive value representing the cost (outflow for buyer) or
/// proceeds (inflow for seller).
pub fn calculate_trade_cost(supply: f64, amount: f64) -> f64 {
    let final_supply = supply + amount;
    let cost = integral_price(final_supply) - integral_price(supply);
    // Ensure cost is positive (it represents the absolute value of the change in collateral)
    cost.abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    const TOLERANCE: f64 = 1e-9; // Define tolerance for float comparisons

    #[test]
    fn test_get_price_positive_supply() {
        assert_eq!(get_price(0.0), 1.0); // Starting price
        assert_eq!(get_price(1.0), 2.0);
        assert_eq!(get_price(4.0), 3.0);
        assert_eq!(get_price(9.0), 4.0);
        // Add more test cases as needed
    }

    #[test]
    fn test_get_price_negative_supply() {
        assert_eq!(get_price(-1.0), 1.0 / 2.0); // 0.5
        assert_eq!(get_price(-4.0), 1.0 / 3.0); // ~0.333
        assert_eq!(get_price(-9.0), 1.0 / 4.0); // 0.25
        // Add more test cases as needed
    }

    #[test]
    fn test_get_price_zero_supply() {
        assert_eq!(get_price(0.0), 1.0);
    }

    #[test]
    fn test_integral_price() {
        assert!((integral_price(0.0f64) - 0.0f64).abs() < TOLERANCE);
        // Integral(1) = 1 + (2/3)*1 = 5/3
        assert!((integral_price(1.0f64) - (5.0f64 / 3.0f64)).abs() < TOLERANCE);
        // Integral(4) = 4 + (2/3)*4^1.5 = 4 + (2/3)*8 = 4 + 16/3 = 28/3
        assert!((integral_price(4.0f64) - (28.0f64 / 3.0f64)).abs() < TOLERANCE);

        // Integral(-1): u=1. -> -2*(1 - ln(2))
        assert!((integral_price(-1.0f64) - (-2.0f64 * (1.0f64 - 2.0f64.ln()))).abs() < TOLERANCE);
        // Integral(-4): u=2. -> -2*(2 - ln(3))
        assert!((integral_price(-4.0f64) - (-2.0f64 * (2.0f64 - 3.0f64.ln()))).abs() < TOLERANCE);
    }

    #[test]
    fn test_calculate_trade_cost() {
        // Buy 1 share starting from 0 supply: cost = I(1) - I(0) = 5/3 - 0 = 5/3
        assert!((calculate_trade_cost(0.0, 1.0) - (5.0f64 / 3.0f64)).abs() < TOLERANCE);
        // Buy 3 shares starting from 1 supply: cost = I(4) - I(1) = 28/3 - 5/3 = 23/3
        assert!((calculate_trade_cost(1.0, 3.0) - (23.0f64 / 3.0f64)).abs() < TOLERANCE);

        // Sell 1 share starting from 0 supply: cost = I(0) - I(-1) = 0 - (-2*(1-ln(2))) = 2*(1-ln(2))
        assert!((calculate_trade_cost(0.0, -1.0) - (2.0f64 * (1.0f64 - 2.0f64.ln()))).abs() < TOLERANCE);
        // Sell 3 shares starting from -1 supply: cost = I(-1) - I(-4) = -2*(1-ln(2)) - (-2*(2-ln(3)))
        // = -2 + 2ln2 + 4 - 2ln3 = 2 + 2ln2 - 2ln3 = 2 * (1 + ln(2/3))
        let expected_cost = 2.0f64 * (1.0f64 + (2.0f64/3.0f64).ln());
        assert!((calculate_trade_cost(-1.0, -3.0) - expected_cost.abs()).abs() < TOLERANCE);

        // Buy 1 share starting from -1 supply: cost = I(0) - I(-1) = 0 - (-2*(1-ln(2)))
        assert!((calculate_trade_cost(-1.0, 1.0) - (2.0f64 * (1.0f64 - 2.0f64.ln())).abs()).abs() < TOLERANCE);

         // Sell 1 share starting from 1 supply: cost = I(1) - I(0) = 5/3 - 0
        assert!((calculate_trade_cost(1.0, -1.0) - (5.0f64 / 3.0f64).abs()).abs() < TOLERANCE);
    }
} 