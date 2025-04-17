use chrono::Utc;
use uuid::Uuid;
use warp::filters::ws::Message;

use super::state::AppState;
use super::models::{ClientMessage, ServerMessage, Post, UserPositionDetail};
use super::constants::{EPSILON, INITIAL_BALANCE};
use super::bonding_curve::{get_price, calculate_cost};
use super::calculations::{calculate_average_price, calculate_unrealized_pnl};
use super::websocket::{send_to_client, broadcast_message, broadcast_market_and_position_updates};

// Helper function to initialize user state if it doesn't exist
fn ensure_user_state_exists(user_id: &str, state: &AppState) {
    // Use entry API to avoid multiple lookups and handle concurrent initialization safely
    state.user_balances.entry(user_id.to_string()).or_insert(INITIAL_BALANCE);
    state.user_realized_pnl.entry(user_id.to_string()).or_insert(0.0);
    // Initialize stored exposure
    state.user_exposure.entry(user_id.to_string()).or_insert(0.0);
}

// Helper function to calculate total unrealized PNL for a user
pub fn calculate_total_unrealized_pnl(user_id: &str, state: &AppState) -> f64 {
    let mut total_urpnl = 0.0;
    if let Some(user_positions) = state.user_positions.get(user_id) {
        for position_entry in user_positions.iter() {
            let post_id = position_entry.key();
            let position = position_entry.value();
            if let Some(post) = state.posts.get(post_id) {
                if let Some(current_price) = post.price {
                    total_urpnl += calculate_unrealized_pnl(position, current_price);
                }
            }
        }
    }
    total_urpnl
}

// Helper function to send a comprehensive user state update
async fn send_user_sync_update(user_id: &str, client_id: Uuid, state: &AppState) {
    let balance = state.user_balances.get(user_id).map_or(INITIAL_BALANCE, |v| *v.value());
    let realized_pnl = state.user_realized_pnl.get(user_id).map_or(0.0, |v| *v.value());
    // Read stored exposure
    let exposure = state.user_exposure.get(user_id).map_or(0.0, |v| *v.value());

    let mut position_details = Vec::new();
    let mut total_unrealized_pnl = 0.0;

    if let Some(user_positions_map) = state.user_positions.get(user_id) {
        for entry in user_positions_map.iter() {
            let post_id = *entry.key();
            let position = entry.value();
            if let Some(post) = state.posts.get(&post_id) {
                 if let Some(current_price) = post.price {
                    let unrealized_pnl = calculate_unrealized_pnl(position, current_price);
                    total_unrealized_pnl += unrealized_pnl;
                    position_details.push(super::models::PositionDetail {
                        post_id,
                        size: position.size,
                        average_price: calculate_average_price(position),
                        unrealized_pnl,
                    });
                 }
            }
        }
    }

    let equity = balance + realized_pnl + total_unrealized_pnl;

    let sync_msg = ServerMessage::UserSync {
        balance,
        exposure, // Use stored exposure
        equity,
        positions: position_details,
        total_realized_pnl: realized_pnl,
    };
    send_to_client(client_id, sync_msg, state).await;
}

pub async fn handle_client_message(
    client_id: Uuid,
    user_id: &str,
    msg: Message,
    state: &AppState,
) {
    // Ensure user state exists before handling any message
    ensure_user_state_exists(user_id, state);

    if let Ok(text) = msg.to_str() {
        match serde_json::from_str::<ClientMessage>(text) {
            Ok(client_msg) => {
                 println!(
                    "User {} ({}) request: {:?}",
                    user_id, client_id, client_msg
                );

                match client_msg {
                    ClientMessage::CreatePost { content } => {
                        handle_create_post(client_id, user_id, content, state).await;
                    }
                    ClientMessage::Buy { post_id, quantity } => {
                        handle_buy(client_id, user_id, post_id, quantity, state).await;
                        // Send a full sync after the operation
                        send_user_sync_update(user_id, client_id, state).await;
                    }
                     ClientMessage::Sell { post_id, quantity } => {
                        handle_sell(client_id, user_id, post_id, quantity, state).await;
                        // Send a full sync after the operation
                        send_user_sync_update(user_id, client_id, state).await;
                    }
                }
            }
            Err(e) => {
                eprintln!("Deserialize error for client_id={}: {}, err={}", client_id, text, e);
                send_to_client(client_id, ServerMessage::Error { message: format!("Invalid message format: {}", e) }, state).await;
            }
        }
    } else if msg.is_ping() {
        // Ping/Pong handled automatically by Warp
    } else if msg.is_close() {
        // Close frame handled by the loop exiting in handle_connection
    } else {
        // Ignore binary messages etc.
    }
}

async fn handle_create_post(
    _client_id: Uuid, // Not strictly needed for create post logic itself
    user_id: &str,
    content: String,
    state: &AppState,
) {
    let new_post_id = Uuid::new_v4();
    let initial_price = get_price(0.0);
    let new_post = Post {
        id: new_post_id,
        user_id: user_id.to_string(),
        content,
        timestamp: Utc::now(),
        supply: 0.0,
        price: Some(initial_price),
    };
    state.posts.insert(new_post_id, new_post.clone());
    println!(
        "-> Post {} created (Price: {:.6}, Supply: 0.0)",
        new_post_id, initial_price
    );
    let broadcast_msg = ServerMessage::NewPost { post: new_post };
    broadcast_message(broadcast_msg, state).await;
}

async fn handle_buy(
    client_id: Uuid,
    user_id: &str,
    post_id: Uuid,
    quantity: f64,
    state: &AppState,
) {
     if quantity <= EPSILON {
        println!("-> Buy FAIL: Quantity {} must be positive", quantity);
        send_to_client(client_id, ServerMessage::Error { message: format!("Buy quantity ({:.6}) must be positive", quantity) }, state).await;
        return;
    }

    ensure_user_state_exists(user_id, state); // Ensure state exists before trade

    // --- Read needed state (minimal locking) ---
    let maybe_post_data = state.posts.get(&post_id).map(|p| (p.supply, p.price));
    let current_balance = *state.user_balances.get(user_id).unwrap().value();
    let current_realized_pnl = *state.user_realized_pnl.get(user_id).unwrap().value();
    let current_exposure = *state.user_exposure.get(user_id).unwrap().value();
    let user_position_val = state.user_positions.get(user_id)
        .and_then(|positions| positions.get(&post_id).map(|pos_detail| pos_detail.clone()));

    // --- Perform checks and calculations ---
    let (current_supply, current_price_opt) = match maybe_post_data {
        Some((supply, price_opt)) => (supply, price_opt),
        None => {
            println!("-> Buy FAIL: Post {} not found", post_id);
            send_to_client(client_id, ServerMessage::Error { message: format!("Post {} not found", post_id) }, state).await;
            return;
        }
    };
    let price_at_trade_start = current_price_opt.unwrap_or_else(|| get_price(current_supply));
    let new_supply = current_supply + quantity;
    let trade_cost = calculate_cost(current_supply, new_supply);
    if trade_cost.is_nan() {
        println!("-> Buy FAIL: Cost calculation resulted in NaN (Supplies: {} -> {})", current_supply, new_supply);
        send_to_client(client_id, ServerMessage::Error { message: "Internal error calculating trade cost.".to_string() }, state).await;
        return;
    }

    // --- Calculate Change in Exposure (Cost Basis Method) ---
    let old_position = user_position_val.unwrap_or_default();
    let old_size = old_position.size;
    let mut delta_exposure = 0.0;

    if old_size < -EPSILON { // Covering a short
        let reduction_amount = quantity.min(old_size.abs());
        if reduction_amount > EPSILON {
            let avg_short_basis_per_share = if old_size.abs() > EPSILON { old_position.total_cost_basis / old_size } else { 0.0 };
            let exposure_reduction = reduction_amount * avg_short_basis_per_share.abs();
            delta_exposure -= exposure_reduction; // Apply reduction directly
            println!(
                "   -> Exposure Change (Buy Cover): Reducing exposure by {:.6} (Reduction: {:.6}, Avg Short Basis/Share: {:.6})",
                exposure_reduction, reduction_amount, avg_short_basis_per_share
            );
        }
    }

    // Check if the buy opens/increases a long position
    if old_size >= -EPSILON { // Started flat or long
        delta_exposure += trade_cost.abs();
        println!(
            "   -> Exposure Change (Buy Open/Increase Long): Increasing exposure by {:.6} (Trade Cost: {:.6})",
            trade_cost.abs(), trade_cost
        );
    } else if quantity > old_size.abs() { // Covered short AND opened long
        let supply_at_zero_crossing = current_supply + old_size.abs();
        let cost_for_long_part = calculate_cost(supply_at_zero_crossing, new_supply);
        delta_exposure += cost_for_long_part.abs();
         println!(
            "   -> Exposure Change (Buy Open Long Part): Increasing exposure by {:.6} (Cost for Long Part: {:.6})",
            cost_for_long_part.abs(), cost_for_long_part
        );
    }

    // --- Potential Exposure Check ---
    let potential_exposure_after_trade = current_exposure + delta_exposure;
    let available_collateral = current_balance + current_realized_pnl;
    if potential_exposure_after_trade > available_collateral + EPSILON { 
         println!(
            "-> Buy FAIL: User {} Insufficient Collateral for Potential Exposure. Required Exposure: {:.6}, Current Exposure: {:.6}, Delta: {:.6}, Available Collateral: {:.6}",
            user_id, potential_exposure_after_trade, current_exposure, delta_exposure, available_collateral
        );
        send_to_client(client_id, ServerMessage::Error { message: format!("Insufficient collateral for potential exposure {:.6}. Available: {:.6}", potential_exposure_after_trade, available_collateral) }, state).await;
        return;
    }
    // --- End Potential Exposure Check ---

    // --- PnL Calculation ---
    let mut realized_pnl_for_trade = 0.0;
    let mut basis_change_for_short_cover = 0.0;
    if old_size < -EPSILON { // Covering a short position
        let reduction_amount = quantity.min(old_size.abs());
        if reduction_amount > EPSILON {
            let avg_short_basis_per_share = if old_size.abs() > EPSILON { old_position.total_cost_basis / old_size } else { 0.0 }; // Basis is negative, size is negative -> positive avg price
            let cost_for_reduction = calculate_cost(current_supply, current_supply + reduction_amount);
            // basis_change represents the magnitude of the basis removed (positive value)
            basis_change_for_short_cover = avg_short_basis_per_share * reduction_amount; 
            // Correct PnL = Proceeds (abs basis change) - Cost
            realized_pnl_for_trade = basis_change_for_short_cover - cost_for_reduction;
            // PnL Print moved to inside lock
        }
    }

    // --- Acquire locks and update state ---
    let final_price;
    let final_position;
    let final_total_realized_pnl;
    let final_exposure;

    { // Scope for mutable references
        // --- Update Post State ---
        let mut post_entry = match state.posts.get_mut(&post_id) {
             Some(entry) => entry,
             None => return, // Should not happen due to earlier check, but good practice
         };
         let post = &mut *post_entry;
         post.supply = new_supply;
         final_price = get_price(new_supply);
         post.price = Some(final_price);

        // --- Update User Balance (NO CHANGE) ---
        // Balance represents lifetime deposits/withdrawals, not affected by trading cost/proceeds.
        // let mut balance_entry = state.user_balances.entry(user_id.to_string()).or_insert(INITIAL_BALANCE);
        // *balance_entry -= trade_cost; // REMOVED
        // final_balance = *balance_entry;

        // --- Update User Position ---
        let user_positions_for_user = state.user_positions.entry(user_id.to_string()).or_default();
        let mut user_position = user_positions_for_user.entry(post_id).or_insert_with(UserPositionDetail::default);
        user_position.size += quantity;

        // --- Update Position Cost Basis ---
        if old_size < -EPSILON { // Covering a short
            // Basis change represents the magnitude of the basis removed (calculated as positive).
            // Original short basis is negative. Add the positive magnitude to move towards zero.
             user_position.total_cost_basis += basis_change_for_short_cover; // CORRECTED: Was -=
             // Note: PnL for the cover is calculated and realized separately.
        }

        // If opening/increasing long, add the cost of the *newly opened* long portion
        if old_size >= -EPSILON { // Started flat or long
            user_position.total_cost_basis += trade_cost;
            println!("   -> Adding Long Basis (Flat/Long Start): Cost: {:.6}, Total Basis: {:.6}",
                    trade_cost, user_position.total_cost_basis);
        } else if quantity > old_size.abs() { // Covered short AND opened long
            // Only add the cost associated with the part that *opens* the long position
            let long_opening_quantity = quantity - old_size.abs();
            let supply_at_zero_crossing = current_supply + old_size.abs(); // Supply when position becomes 0
            let cost_for_long_part = calculate_cost(supply_at_zero_crossing, new_supply);
            user_position.total_cost_basis += cost_for_long_part;
            println!("   -> Adding Long Basis (Short Cover + Open Long): Qty: {:.6}, Cost for Long Part (Supply {:.6} -> {:.6}): {:.6}, Total Basis: {:.6}",
                    long_opening_quantity, supply_at_zero_crossing, new_supply, cost_for_long_part, user_position.total_cost_basis);
        }

        // --- Update Realized PNL ---
        if realized_pnl_for_trade.abs() > EPSILON {
            println!(
                "   -> Realizing PNL (Buy Cover): {:.6} for User {}",
                realized_pnl_for_trade, user_id
            );
            let mut total_pnl = state.user_realized_pnl.entry(user_id.to_string()).or_insert(0.0);
            *total_pnl += realized_pnl_for_trade;
            final_total_realized_pnl = *total_pnl;
        } else {
            final_total_realized_pnl = *state.user_realized_pnl.get(user_id).unwrap().value(); // Assumes exists due to ensure_user_state_exists
        }

        // --- Update Stored Exposure ---
        let mut exposure_entry = state.user_exposure.entry(user_id.to_string()).or_insert(0.0);
        *exposure_entry += delta_exposure; // Apply calculated change
         // Ensure exposure doesn't go negative due to float issues
        if *exposure_entry < 0.0 { *exposure_entry = 0.0; }
        final_exposure = *exposure_entry;

        // --- Cleanup near-zero positions ---
         if user_position.size.abs() < EPSILON {
             println!("   -> Position size is near zero ({:.8}) after trade, resetting basis and size.", user_position.size);
             user_position.size = 0.0;
             user_position.total_cost_basis = 0.0;
         }

        final_position = user_position.clone();

    } // Mutable references are dropped here

    // --- Log results ---
    let current_balance_for_log = state.user_balances.get(user_id).map_or(0.0, |v| *v.value());

     println!(
        "-> Buy OK (Qty: {:.6}, Cost: {:.6}): Post {} (Supply: {:.6}, Prc: {:.6}), User {} (Pos: {:.6}, RPnlTrade: {:.6}, Bal: {:.6}, TotRPnl: {:.6}, FinalExp: {:.6})",
        quantity, trade_cost, post_id, new_supply, final_price, user_id, final_position.size,
        realized_pnl_for_trade,
        current_balance_for_log, final_total_realized_pnl, final_exposure
    );

    // --- Send Granular Updates (Optional - covered by send_user_sync_update below) ---
    // send_to_client(client_id, ServerMessage::BalanceUpdate { balance: final_balance }, state).await; // Balance doesn't change per trade
    // send_to_client(client_id, ServerMessage::RealizedPnlUpdate { total_realized_pnl: final_total_realized_pnl }, state).await;
    // send_to_client(client_id, ServerMessage::ExposureUpdate { exposure: final_exposure }, state).await;
    // send_to_client(client_id, ServerMessage::PositionUpdate {
    //     post_id, size: final_position.size,
    //     average_price: calculate_average_price(&final_position), // Send signed avg price? Or always positive? Let's keep positive.
    //     calculate_unrealized_pnl(&final_position, final_price)
    //  }, state).await;
    // let equity = current_balance_for_log + final_total_realized_pnl + calculate_total_unrealized_pnl(user_id, state);
    // send_to_client(client_id, ServerMessage::EquityUpdate { equity }, state).await;

    // Remove old margin update
    // let new_margin = calculate_user_margin(user_id, state);
    // send_to_client(client_id, ServerMessage::MarginUpdate { margin: new_margin }, state).await;

    // --- Broadcast Market and Position Updates (Needs review) ---
    // This broadcasts market state and potentially *other* users' position updates if they hold the same post.
    // We might not need to broadcast individual position updates widely, only market updates.
    // The send_user_sync_update called after this handler will update the acting user's state.
    broadcast_market_and_position_updates(post_id, final_price, new_supply, client_id, state).await;

    // Note: send_user_sync_update will be called after this function returns in handle_client_message
}

async fn handle_sell(
    client_id: Uuid,
    user_id: &str,
    post_id: Uuid,
    quantity: f64,
    state: &AppState,
) {
     if quantity <= EPSILON {
         println!("-> Sell FAIL: Quantity {} must be positive", quantity);
        send_to_client(client_id, ServerMessage::Error { message: format!("Sell quantity ({:.6}) must be positive", quantity) }, state).await;
        return;
    }

    ensure_user_state_exists(user_id, state); // Ensure state exists before trade

    // --- Read needed state (minimal locking) ---
    let maybe_post_data = state.posts.get(&post_id).map(|p| (p.supply, p.price));
    let current_balance = *state.user_balances.get(user_id).unwrap().value();
    let current_realized_pnl = *state.user_realized_pnl.get(user_id).unwrap().value();
    let current_exposure = *state.user_exposure.get(user_id).unwrap().value();
    let user_position_val = state.user_positions.get(user_id)
        .and_then(|positions| positions.get(&post_id).map(|pos_detail| pos_detail.clone()));

    // --- Perform checks and calculations ---
    let (current_supply, _) = match maybe_post_data {
        Some((supply, price_opt)) => (supply, price_opt),
        None => {
            println!("-> Sell FAIL: Post {} not found", post_id);
            send_to_client(client_id, ServerMessage::Error { message: format!("Post {} not found", post_id) }, state).await;
            return;
        }
    };
    let new_supply = current_supply - quantity;
    let trade_proceeds = calculate_cost(new_supply, current_supply);
    if trade_proceeds.is_nan() { // Proceeds should be non-negative
        println!(
            "-> Sell FAIL: Proceeds calculation invalid (Supplies: {} -> {}, Proceeds: {})",
            current_supply, new_supply, trade_proceeds
        );
        send_to_client(client_id, ServerMessage::Error { message: "Internal error calculating trade proceeds.".to_string() }, state).await;
        return;
    }

    let old_position = user_position_val.unwrap_or_default();
    let old_size = old_position.size;

    // --- Calculate Change in Exposure (Cost Basis Method) ---
    let mut delta_exposure = 0.0;

    if old_size > EPSILON { // Closing out a long position
        let reduction_amount = quantity.min(old_size);
        if reduction_amount > EPSILON {
            let avg_long_basis_per_share = if old_size.abs() > EPSILON { old_position.total_cost_basis / old_size } else { 0.0 };
            let exposure_reduction = reduction_amount * avg_long_basis_per_share.abs();
            delta_exposure -= exposure_reduction; // Apply reduction directly
            println!(
                "   -> Exposure Change (Sell Close Long): Reducing exposure by {:.6} (Reduction: {:.6}, Avg Long Basis/Share: {:.6})",
                exposure_reduction, reduction_amount, avg_long_basis_per_share
            );
        }
    }

    // Check if the sell opens/increases a short position
    if old_size <= EPSILON { // Started flat or short
        delta_exposure += trade_proceeds.abs();
        println!(
            "   -> Exposure Change (Sell Open/Increase Short): Increasing exposure by {:.6} (Trade Proceeds: {:.6})",
            trade_proceeds.abs(), trade_proceeds
        );
    } else if quantity > old_size { // Closed long AND opened short
        let supply_at_zero_crossing = current_supply - old_size;
        let proceeds_for_short_part = calculate_cost(new_supply, supply_at_zero_crossing);
        delta_exposure += proceeds_for_short_part.abs();
        println!(
            "   -> Exposure Change (Sell Open Short Part): Increasing exposure by {:.6} (Proceeds for Short Part: {:.6})",
            proceeds_for_short_part.abs(), proceeds_for_short_part
        );
    }

    // --- Potential Exposure Check ---
    let potential_exposure_after_trade = current_exposure + delta_exposure;
    let available_collateral = current_balance + current_realized_pnl;
    if potential_exposure_after_trade > available_collateral + EPSILON {
         println!(
            "-> Sell FAIL: User {} Insufficient Collateral for Potential Exposure. Required Exposure: {:.6}, Current Exposure: {:.6}, Delta: {:.6}, Available Collateral: {:.6}",
            user_id, potential_exposure_after_trade, current_exposure, delta_exposure, available_collateral
        );
        send_to_client(client_id, ServerMessage::Error { message: format!("Insufficient collateral for potential exposure {:.6}. Available: {:.6}", potential_exposure_after_trade, available_collateral) }, state).await;
        return;
    }
    // --- End Potential Exposure Check ---

    // --- PnL Calculation ---
    let mut realized_pnl_for_trade = 0.0;
    let mut basis_removed = 0.0; // Basis change when closing long
    if old_size > EPSILON { // Closing out a long position
        let reduction_amount = quantity.min(old_size);
        if reduction_amount > EPSILON {
            let avg_long_basis_per_share = if old_size.abs() > EPSILON { old_position.total_cost_basis / old_size } else { 0.0 };
            let proceeds_for_reduction = calculate_cost(current_supply - reduction_amount, current_supply);
            basis_removed = avg_long_basis_per_share * reduction_amount;
            realized_pnl_for_trade = proceeds_for_reduction - basis_removed;
            // PnL Print moved to inside lock
        }
    }

    // --- Acquire locks and update state ---
    let final_price;
    let final_position;
    let final_total_realized_pnl;
    let final_exposure;

    { // Scope for mutable references
        // --- Update Post State ---
        let mut post_entry = match state.posts.get_mut(&post_id) {
             Some(entry) => entry,
             None => return, // Should not happen
         };
         let post = &mut *post_entry;
         post.supply = new_supply;
         final_price = get_price(new_supply);
         post.price = Some(final_price);

        // --- Update User Balance (NO CHANGE) ---
        // Balance represents lifetime deposits/withdrawals, not affected by trading cost/proceeds.
        // let mut balance_entry = state.user_balances.entry(user_id.to_string()).or_insert(INITIAL_BALANCE);
        // *balance_entry += trade_proceeds; // REMOVED
        // final_balance = *balance_entry;

        // --- Update User Position ---
        let user_positions_for_user = state.user_positions.entry(user_id.to_string()).or_default();
        let mut user_position = user_positions_for_user.entry(post_id).or_insert_with(UserPositionDetail::default);
        user_position.size -= quantity;

        // --- Update Position Cost Basis ---
        if old_size > EPSILON { 
             user_position.total_cost_basis -= basis_removed;
             println!(
                "   -> Selling long: Pnl: {:.6}", // Log PnL here
                 realized_pnl_for_trade
             );
         }
        if old_size <= EPSILON { 
            user_position.total_cost_basis -= trade_proceeds; 
        } else if quantity > old_size { 
            let supply_at_zero_crossing = current_supply - old_size; 
            let proceeds_for_short_part = calculate_cost(new_supply, supply_at_zero_crossing); 
            user_position.total_cost_basis -= proceeds_for_short_part;
        }

        // --- Update Realized PNL ---
        if realized_pnl_for_trade.abs() > EPSILON {
            let mut total_pnl = state.user_realized_pnl.entry(user_id.to_string()).or_insert(0.0);
            *total_pnl += realized_pnl_for_trade;
            final_total_realized_pnl = *total_pnl;
         } else {
            final_total_realized_pnl = *state.user_realized_pnl.get(user_id).unwrap().value();
        }

        // --- Update Stored Exposure ---
        let mut exposure_entry = state.user_exposure.entry(user_id.to_string()).or_insert(0.0);
        *exposure_entry += delta_exposure; // Apply calculated change
         // Ensure exposure doesn't go negative due to float issues
        if *exposure_entry < 0.0 { *exposure_entry = 0.0; }
        final_exposure = *exposure_entry;

        // --- Cleanup near-zero positions ---
         if user_position.size.abs() < EPSILON {
             println!("   -> Position size is near zero ({:.8}) after trade, resetting basis and size.", user_position.size);
             user_position.size = 0.0;
             user_position.total_cost_basis = 0.0;
             // If position is zero, exposure should ideally be zero.
             // Check if stored exposure matches calculated exposure after cleanup?
             // let calculated_exp_after_cleanup = calculate_user_exposure(user_id, state); // If helper existed
             // if (*exposure_entry - calculated_exp_after_cleanup).abs() > EPSILON * 100.0 { ... log warning ... }
         }

        final_position = user_position.clone();
    } // Mutable references are dropped here

    // --- Log results ---
    let current_balance_for_log = state.user_balances.get(user_id).map_or(0.0, |v| *v.value());

    // Updated log message
     println!(
        "-> Sell OK (Qty: {:.6}, Proceeds: {:.6}): Post {} (Supply: {:.6}, Prc: {:.6}), User {} (Pos: {:.6}, RPnlTrade: {:.6}, Bal: {:.6}, TotRPnl: {:.6}, FinalExp: {:.6})",
        quantity, trade_proceeds, post_id, new_supply, final_price, user_id, final_position.size,
        realized_pnl_for_trade,
        current_balance_for_log, final_total_realized_pnl, final_exposure
    );

    // --- Send Granular Updates (Optional - covered by send_user_sync_update below) ---
    // send_to_client(client_id, ServerMessage::BalanceUpdate { balance: final_balance }, state).await; // Balance doesn't change per trade
    // send_to_client(client_id, ServerMessage::RealizedPnlUpdate { total_realized_pnl: final_total_realized_pnl }, state).await;
    // send_to_client(client_id, ServerMessage::ExposureUpdate { exposure: final_exposure }, state).await;
    // send_to_client(client_id, ServerMessage::PositionUpdate {
    //     post_id, size: final_position.size,
    //     average_price: calculate_average_price(&final_position),
    //     calculate_unrealized_pnl(&final_position, final_price)
    //  }, state).await;
    // let equity = current_balance_for_log + final_total_realized_pnl + calculate_total_unrealized_pnl(user_id, state);
    // send_to_client(client_id, ServerMessage::EquityUpdate { equity }, state).await;

    // Remove old margin update
    // let new_margin = calculate_user_margin(user_id, state);
    // send_to_client(client_id, ServerMessage::MarginUpdate { margin: new_margin }, state).await;

    // --- Broadcast Market and Position Updates (Needs review) ---
    broadcast_market_and_position_updates(post_id, final_price, new_supply, client_id, state).await;

    // Note: send_user_sync_update will be called after this function returns in handle_client_message
} 