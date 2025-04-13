use super::state::AppState;
use super::models::UserPositionDetail;
use super::constants::{EPSILON, INITIAL_BALANCE};
use super::bonding_curve::get_price;

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