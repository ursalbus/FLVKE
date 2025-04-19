use super::state::AppState;
use super::models::UserPositionDetail;
use super::constants::{EPSILON, INITIAL_BALANCE, BONDING_CURVE_EPSILON};
use super::bonding_curve::{get_price, calculate_smooth_cost};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Bound::{Included, Excluded, Unbounded};
use ordered_float::OrderedFloat;
use uuid::Uuid;
use num_traits::Float; // For f64::min/max if needed, or use std::cmp::min/max
use std::cmp::Ordering;

// --- Calculation Helpers ---

pub fn calculate_average_price(position: &UserPositionDetail) -> f64 {
    if position.size.abs() < EPSILON {
        0.0
    } else {
        position.total_cost_basis / position.size
    }
}

pub fn calculate_unrealized_pnl(
    position: &UserPositionDetail,
    current_market_price: f64,
) -> f64 {
     if position.size.abs() < EPSILON {
        0.0
    } else {
        let avg_price = calculate_average_price(position);
        (current_market_price - avg_price) * position.size
    }
}

// Calculate the supply level at which a user would be liquidated for a specific post.
// Assumes this is the *only* position impacting their equity for simplicity.
// Returns None if liquidation is impossible (e.g., requires non-positive price).
pub fn calculate_liquidation_supply(
    balance: f64,
    total_realized_pnl: f64,
    position_size: f64,
    average_entry_price: f64,
) -> Option<f64> {
    if position_size.abs() < EPSILON {
        return None; // No position, no liquidation threshold
    }

    let collateral = balance + total_realized_pnl;

    // Target price P(s_liq) where equity = 0
    // collateral + (P(s_liq) - average_entry_price) * position_size = 0
    // P(s_liq) = average_entry_price - collateral / position_size
    // Avoid division by zero edge case although checked earlier
    if position_size.abs() < EPSILON { return None; }
    let target_price = average_entry_price - collateral / position_size;

    // Price must be positive
    if target_price <= 0.0 + BONDING_CURVE_EPSILON { // Add epsilon for safety
        return None; // Liquidation would require non-positive price, impossible
    }

    // Now find supply 's' such that P(s) = target_price
    if (target_price - 1.0).abs() < BONDING_CURVE_EPSILON {
        // Target price is essentially 1.0
        Some(0.0)
    } else if target_price > 1.0 {
        // Positive supply curve: P(s) = 1 + sqrt(s)
        // sqrt(s) = target_price - 1
        // s = (target_price - 1)^2
        let s_liq = (target_price - 1.0).powi(2);
        // Ensure result is positive as expected
        if s_liq >= 0.0 { Some(s_liq) } else { Some(0.0) } // Clamp to 0 if float error occurs
    } else { // target_price < 1.0
        // Negative supply curve: P(s) = 1 / (1 + sqrt(|s|))
        // 1 + sqrt(|s|) = 1 / target_price
        // sqrt(|s|) = (1.0 / target_price) - 1.0
        // Protect against sqrt of negative if target_price >= 1 due to float error
        let base = (1.0 / target_price) - 1.0;
        if base < 0.0 { return Some(0.0); } // Should have been caught by target_price > 1 check

        // |s| = ((1.0 / target_price) - 1.0)^2
        let s_liq_abs = base.powi(2);
        let s_liq = -s_liq_abs;
         // Ensure result is negative as expected
        if s_liq <= 0.0 { Some(s_liq) } else { Some(0.0) } // Clamp to 0 if float error occurs
    }
}

// --- Effective Cost Calculation --- 

#[derive(Debug)]
pub struct EffectiveTradeResult {
    pub effective_cost: f64, // Positive=Cost to buyer, Negative=Proceeds to seller
    pub final_supply: f64,
    pub liquidated_users: Vec<(String, f64)>, // Vec of (UserId, ForcedTradePnL)
}

// Calculates the effective cost/proceeds and final supply for a trade,
// using a single-pass segmented integration over liquidation thresholds.
pub fn calculate_effective_cost_and_final_supply(
    start_supply: f64,
    trade_quantity: f64, // Positive for buy, negative for sell
    post_id: Uuid,
    state: &AppState,
) -> Result<EffectiveTradeResult, String> {

    if trade_quantity.abs() < EPSILON {
        return Ok(EffectiveTradeResult {
            effective_cost: 0.0,
            final_supply: start_supply,
            liquidated_users: Vec::new(),
        });
    }

    let mut current_s = start_supply;
    let mut remaining_qty_a = trade_quantity;
    let mut effective_cost = 0.0;
    let mut final_supply_calc = start_supply; // Start with initial, will add Qty_A and LiqSizes
    let mut liquidated_user_details: Vec<(String, f64, f64)> = Vec::new(); // (UserId, Cost_Unwind, Size_Unwind)

    // Get the thresholds map
    let thresholds_map = match state.liquidation_thresholds.get(&post_id) {
        Some(map_ref) => map_ref.value().clone(), // Clone the BTreeMap for processing
        None => {
            println!("Warning: Liquidation thresholds not found for post {}. Proceeding with smooth curve only.", post_id);
            BTreeMap::new()
        }
    };

    // Determine iteration direction and create iterator
    let direction = trade_quantity.signum(); // 1.0 for buy, -1.0 for sell

    // Loop until trader's quantity is fully processed
    while remaining_qty_a.abs() > EPSILON {
        // Find the next threshold in the direction of trade
        let next_threshold_opt = if direction > 0.0 { // Buying
            thresholds_map.range((Excluded(OrderedFloat(current_s)), Unbounded)).next()
        } else { // Selling
            thresholds_map.range((Unbounded, Excluded(OrderedFloat(current_s)))).next_back()
        };

        let supply_limit_for_segment = match next_threshold_opt {
            Some((s_liq_key, _)) => s_liq_key.into_inner(),
            None => current_s + remaining_qty_a, // No more thresholds, trade goes to its end
        };

        // Determine how much supply change happens in this segment
        // Max change is either to the next threshold or the remaining user quantity
        let delta_s_to_limit = supply_limit_for_segment - current_s;
        let delta_s_this_segment = if direction * delta_s_to_limit > direction * remaining_qty_a {
             // Trade finishes before reaching the limit
             remaining_qty_a
        } else {
             // Trade reaches or passes the limit (delta_s_to_limit has correct sign)
             delta_s_to_limit
        };

        let segment_end_s = current_s + delta_s_this_segment;

        // Calculate cost for this smooth segment
        let cost_segment = calculate_smooth_cost(current_s, segment_end_s);
        if cost_segment.is_nan() {
            return Err(format!("Smooth cost calculation failed in segment {} -> {}", current_s, segment_end_s));
        }
        effective_cost += cost_segment;

        // Update state
        current_s = segment_end_s;
        remaining_qty_a -= delta_s_this_segment;

        // Process liquidation if threshold was exactly reached
        if (current_s - supply_limit_for_segment).abs() < EPSILON && next_threshold_opt.is_some() {
            let (s_liq_key, liq_entries) = next_threshold_opt.unwrap(); // We know it's Some
            println!("   - Processing Liq Threshold at Supply {:.4}", s_liq_key.into_inner());
            for (cost_unwind, size_unwind, user_id) in liq_entries {
                effective_cost += *cost_unwind;
                // The actual supply jump happens here
                current_s += *size_unwind;
                 println!("     - Liq User {}: Cost={:.4}, Size={:.4}. New current_s={:.4}", user_id, cost_unwind, size_unwind, current_s);
                liquidated_user_details.push((user_id.clone(), *cost_unwind, *size_unwind));
            }
        }
    }

    // Final supply is the point reached after all segments and jumps
    final_supply_calc = current_s;

    // Calculate PnL for liquidated users
    let mut liquidated_users_pnl = Vec::new();
    for (user_id, cost_unwind, size_unwind) in liquidated_user_details {
        let avg_price = state.user_positions.get(&user_id)
                                .and_then(|m| m.get(&post_id)
                                .as_ref()
                                .map(|p| calculate_average_price(p)))
                                .unwrap_or(0.0);
        let original_basis = avg_price * (-size_unwind);
        let forced_trade_pnl = -cost_unwind - original_basis;
        liquidated_users_pnl.push((user_id, forced_trade_pnl));
    }

    println!("   - Effective Cost Final: {:.4}", effective_cost);
    println!("   - Final Supply Final: {:.4}", final_supply_calc);

    Ok(EffectiveTradeResult {
        effective_cost,
        final_supply: final_supply_calc,
        liquidated_users: liquidated_users_pnl,
    })
}

// --- Margin Calculation Helper ---

pub fn calculate_user_margin(user_id: &str, state: &AppState) -> f64 {
    let balance = state.user_balances.get(user_id).map_or(INITIAL_BALANCE, |b| *b.value());
    let mut total_unrealized_pnl = 0.0;

    if let Some(user_positions_map) = state.user_positions.get(user_id) {
        for position_entry in user_positions_map.iter() {
            let post_id = *position_entry.key();
            let position = position_entry.value();

            if position.size.abs() > EPSILON {
                if let Some(market_post) = state.posts.get(&post_id) {
                    let current_market_price = market_post.price.unwrap_or_else(|| get_price(market_post.supply));
                    total_unrealized_pnl += calculate_unrealized_pnl(position, current_market_price);
                } else {
                    eprintln!("Warning: Post {} not found while calculating margin for user {}", post_id, user_id);
                }
            }
        }
    }
    balance + total_unrealized_pnl
} 