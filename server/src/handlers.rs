use chrono::Utc;
use uuid::Uuid;
use warp::filters::ws::Message;
use std::collections::{BTreeMap, HashMap, HashSet};
use ordered_float::OrderedFloat;
use tokio::time::Instant;

use super::state::{AppState, UserPositions, UserBalances, UserRealizedPnl, Posts, LiquidationThresholds};
use super::models::{ClientMessage, ServerMessage, Post, UserPositionDetail};
use super::constants::{EPSILON, INITIAL_BALANCE};
use super::bonding_curve::{get_price, calculate_smooth_cost};
use super::calculations::{
    calculate_average_price, calculate_unrealized_pnl, calculate_liquidation_supply,
    EffectiveTradeResult, calculate_effective_cost_and_final_supply
};
use super::websocket::{send_to_client, broadcast_message, broadcast_market_and_position_updates};

// Helper function to initialize user state if it doesn't exist
fn ensure_user_state_exists(user_id: &str, state: &AppState) {
    // Use entry API to avoid multiple lookups and handle concurrent initialization safely
    state.user_balances.entry(user_id.to_string()).or_insert(INITIAL_BALANCE);
    state.user_realized_pnl.entry(user_id.to_string()).or_insert(0.0);
    // Initialize stored exposure
    state.user_exposure.entry(user_id.to_string()).or_insert(0.0);
    // Ensure liquidation threshold map exists for posts (handled in update func)
}

// Helper function to calculate total unrealized PNL for a user
pub fn calculate_total_unrealized_pnl(user_id: &str, state: &AppState) -> f64 {
    let mut total_urpnl = 0.0;
    if let Some(user_positions) = state.user_positions.get(user_id) {
        for position_entry in user_positions.iter() {
            let post_id = position_entry.key();
            let position = position_entry.value();
            if let Some(post) = state.posts.get(post_id) {
                // Use the post's stored price if available, otherwise calculate
                let current_price = post.price.unwrap_or_else(|| get_price(post.supply));
                    total_urpnl += calculate_unrealized_pnl(position, current_price);
            }
        }
    }
    total_urpnl
}

// Helper function to calculate total exposure for a user
// Exposure = Sum of absolute value of cost basis for all positions
pub fn calculate_total_exposure(user_id: &str, state: &AppState) -> f64 {
    let mut total_exposure = 0.0;
    if let Some(user_positions) = state.user_positions.get(user_id) {
        for position_entry in user_positions.iter() {
            let position = position_entry.value();
            // Use abs() because basis can be negative for shorts
            total_exposure += position.total_cost_basis.abs(); 
        }
    }
    total_exposure
}

// Helper function to send a comprehensive user state update
async fn send_user_sync_update(user_id: &str, client_id: Uuid, state: &AppState) {
    println!("--- Entering send_user_sync_update for User {} (Client {}) ---", user_id, client_id);
    // Fetch potentially updated values
    println!("send_user_sync_update: Fetching balance...");
    let balance = state.user_balances.get(user_id).map_or(INITIAL_BALANCE, |v| *v.value());
    println!("send_user_sync_update: Fetching realized_pnl...");
    let realized_pnl = state.user_realized_pnl.get(user_id).map_or(0.0, |v| *v.value());
    println!("send_user_sync_update: Fetching exposure...");
    let exposure = state.user_exposure.get(user_id).map_or(0.0, |v| *v.value());
    println!("send_user_sync_update: Bal={:.4}, RPnl={:.4}, Exp={:.4}. Preparing to collect position data...", balance, realized_pnl, exposure);

    // --- Collect position data safely --- 
    let mut collected_positions = Vec::new();
    let mut collection_error: Option<String> = None;

    // Catch potential panics during position access
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        println!("send_user_sync_update: Inside catch_unwind - Attempting state.user_positions.get()...");
        let mut temp_positions = Vec::new();
        if let Some(user_positions_map_ref) = state.user_positions.get(user_id) {
            println!("send_user_sync_update: Inside catch_unwind - Got map reference. Iterating...");
            for entry in user_positions_map_ref.iter() {
                // Clone necessary data: post_id and UserPositionDetail
                temp_positions.push((*entry.key(), entry.value().clone()));
            }
             println!("send_user_sync_update: Inside catch_unwind - Collected {} positions for user {}.", temp_positions.len(), user_id);
        } else {
             println!("send_user_sync_update: Inside catch_unwind - No positions map found for user {}.", user_id);
        }
        temp_positions // Return the collected data if successful
    }));

    match result {
        Ok(positions) => {
            println!("send_user_sync_update: catch_unwind succeeded. Using collected positions.");
            collected_positions = positions;
        }
        Err(panic_payload) => {
            // Log the panic
            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                *s
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.as_str()
            } else {
                "Unknown panic type"
            };
            eprintln!("CRITICAL PANIC CAUGHT in send_user_sync_update while accessing user_positions for user {}: {}", user_id, panic_msg);
            collection_error = Some(format!("Panic accessing position data: {}", panic_msg));
            // collected_positions remains empty
        }
    }
    
    // If collection failed (panic or otherwise), we might still want to send a partial UserSync
    if let Some(err_msg) = collection_error {
       eprintln!("send_user_sync_update: Error occurred during position collection: {}", err_msg);
       // Optionally, send an error message to the client or a UserSync with empty positions?
       // For now, we continue and will send UserSync with empty positions.
    }

    // --- Process collected positions --- 
    println!("send_user_sync_update: Processing collected positions (count: {})...", collected_positions.len());
    let mut position_details = Vec::new();
    let mut total_unrealized_pnl = 0.0;

    // Need user's balance and rpnl for liquidation calc
    let user_balance_for_liq = state.user_balances.get(user_id).map_or(INITIAL_BALANCE, |v| *v.value());
    let user_rpnl_for_liq = state.user_realized_pnl.get(user_id).map_or(0.0, |v| *v.value());

    for (post_id, position_value) in collected_positions {
        println!("send_user_sync_update: Processing collected position for post {}", post_id);
        let current_market_price = state.posts.get(&post_id)
            .map_or(0.0, |p| p.value().price.unwrap_or(0.0));
        println!("send_user_sync_update: Post {}, MarketPrice={:.4}, PosSize={:.4}. Calculating PnL...", post_id, current_market_price, position_value.size);

        // Post existence check might be less critical now, but still good practice
        if let Some(_post) = state.posts.get(&post_id) { 
             let avg_price = calculate_average_price(&position_value);
             if position_value.size.abs() > EPSILON {
                let unrealized_pnl = calculate_unrealized_pnl(&position_value, current_market_price);
                println!("send_user_sync_update: Post {}, AvgPrc={:.4}, uPnL={:.4}. Calculating liq supply...", post_id, avg_price, unrealized_pnl);
                    total_unrealized_pnl += unrealized_pnl;

                // Calculate liquidation supply for this position
                let liquidation_supply = calculate_liquidation_supply(
                    user_balance_for_liq, 
                    user_rpnl_for_liq, 
                    position_value.size, 
                    avg_price
                );
                println!("send_user_sync_update: Post {}, LiqSupply={:?}. Adding detail.", post_id, liquidation_supply);

                    position_details.push(super::models::PositionDetail {
                        post_id,
                    size: position_value.size,
                    average_price: avg_price,
                        unrealized_pnl,
                    liquidation_supply, // Add the calculated value
                    });
             } else {
                println!("send_user_sync_update: Post {}, Size near zero, skipping detail.", post_id);
                 }
        } else {
            eprintln!("Warning: Post {} disappeared while processing collected UserSync data for user {}", post_id, user_id);
        }
    }
    println!("send_user_sync_update: Finished processing collected positions. Total uPnL={:.4}", total_unrealized_pnl);

    let equity = balance + realized_pnl + total_unrealized_pnl;
    println!("send_user_sync_update: Calculated Equity={:.4}. Constructing message...", equity);

    let sync_msg = ServerMessage::UserSync {
        balance,
        exposure,
        equity,
        positions: position_details,
        total_realized_pnl: realized_pnl,
    };
    println!("send_user_sync_update: Message constructed. Serializing...");

    // Find the client sender to send the message
    // Check if client is still connected before sending
    if let Some(client_entry) = state.clients.get(&client_id) {
        let client = client_entry.value();
        if client.user_id == user_id { // Double check user_id match
           println!("send_user_sync_update: Found client entry for {}. Serializing JSON...", client_id);
           match serde_json::to_string(&sync_msg) {
               Ok(msg_str) => {
                   println!("send_user_sync_update: Serialized OK. Sending via MPSC channel...");
                   if let Err(e) = client.sender.send(Ok(warp::filters::ws::Message::text(msg_str))) {
                        eprintln!("Error sending UserSync to client {}: {}", client_id, e);
                   }
                   println!("send_user_sync_update: Sent via MPSC channel (or queued).");
               },
               Err(e) => {
                    // Log serialization error - this could be a source of panic if unhandled floats exist
                    eprintln!("Critical Error: Failed to serialize UserSync for user {}: {}. Message: {:?}", user_id, e, sync_msg);
                    // Consider sending an error message instead? Or just logging.
               }
           }
        } else {
            eprintln!("Error: Client ID {} does not match User ID {} during UserSync send.", client_id, user_id);
        }
    } else {
        println!("send_user_sync_update: Client {} not found (offline?). Skipping send.", client_id);
    }
    println!("--- Exiting send_user_sync_update for User {} (Client {}) ---", user_id, client_id);
}

pub async fn handle_client_message(
    client_id: Uuid,
    user_id: &str,
    msg: warp::filters::ws::Message,
    state: &AppState,
) {
    ensure_user_state_exists(user_id, state);
    if let Ok(text) = msg.to_str() {
        match serde_json::from_str::<ClientMessage>(text) {
            Ok(client_msg) => {
                println!("User {} ({}) request: {:?}", user_id, client_id, client_msg);
                match client_msg {
                    ClientMessage::CreatePost { content } => {
                        println!("handle_client_message: Calling handle_create_post...");
                        let new_post_id = handle_create_post(client_id, user_id, content, state).await;
                        println!("handle_client_message: Returned from handle_create_post. Calling update_liquidation_thresholds...");
                        update_liquidation_thresholds(new_post_id, state).await;
                        println!("handle_client_message: Returned from update_liquidation_thresholds after CreatePost.");
                    }
                    ClientMessage::Buy { post_id, quantity } => {
                        println!("handle_client_message: Calling handle_buy...");
                        handle_buy(client_id, user_id, post_id, quantity, state).await;
                        println!("handle_client_message: Returned from handle_buy. Calling update_liquidation_thresholds...");
                        update_liquidation_thresholds(post_id, state).await;
                        println!("handle_client_message: Returned from update_liquidation_thresholds after Buy.");
                    }
                     ClientMessage::Sell { post_id, quantity } => {
                        println!("handle_client_message: Calling handle_sell...");
                        handle_sell(client_id, user_id, post_id, quantity, state).await;
                        println!("handle_client_message: Returned from handle_sell. Calling update_liquidation_thresholds...");
                        update_liquidation_thresholds(post_id, state).await;
                        println!("handle_client_message: Returned from update_liquidation_thresholds after Sell.");
                    }
                }
            }
            Err(e) => {
                 // Also log deserialization errors
                 eprintln!("Error deserializing client message from {}: {}. Raw text: '{}'", client_id, e, text);
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
    _client_id: Uuid,
    user_id: &str,
    content: String,
    state: &AppState,
) -> Uuid {
    let new_post_id = Uuid::new_v4();
    let initial_price = get_price(0.0);
    let new_post = Post {
        id: new_post_id,
        user_id: user_id.to_string(),
        content,
        timestamp: Utc::now(),
        supply: 0.0,
        price: Some(initial_price),
        // Removed old fields
    };
    // Ensure threshold map exists for the new post, even if empty
    state.liquidation_thresholds.insert(new_post_id, BTreeMap::new());
    state.posts.insert(new_post_id, new_post.clone());
    println!(
        "-> Post {} created (Price: {:.6}, Supply: 0.0)",
        new_post_id, initial_price
    );
    let broadcast_msg = ServerMessage::NewPost { post: new_post };
    broadcast_message(broadcast_msg, state).await;
    new_post_id
}

async fn handle_buy(
    client_id: Uuid,
    trader_user_id: &str,
    post_id: Uuid,
    quantity: f64,
    state: &AppState,
) {
     if quantity <= EPSILON {
        send_to_client(client_id, ServerMessage::Error { message: format!("Buy quantity ({:.6}) must be positive", quantity) }, state).await;
        return;
    }
    ensure_user_state_exists(trader_user_id, state);

    // --- Phase 1: Read Initial State & Calculate Effective Trade ---
    let initial_supply = match state.posts.get(&post_id) {
        Some(post_entry) => post_entry.supply,
        None => { send_to_client(client_id, ServerMessage::Error { message: format!("Post {} not found", post_id) }, state).await; return; }
    };

    let trade_result = match calculate_effective_cost_and_final_supply(initial_supply, quantity, post_id, state) {
        Ok(result) => result,
        Err(e) => { send_to_client(client_id, ServerMessage::Error { message: format!("Trade calculation error: {}", e) }, state).await; return; }
    };

    // --- Phase 2: Collateral Check ---
    let balance = state.user_balances.get(trader_user_id).map_or(INITIAL_BALANCE, |v| *v.value());
    let realized_pnl = state.user_realized_pnl.get(trader_user_id).map_or(0.0, |v| *v.value());
    let available_collateral = balance + realized_pnl;

    // Note: Simplified check
    if trade_result.effective_cost > available_collateral + EPSILON {
        send_to_client(client_id, ServerMessage::Error { message: format!("Insufficient collateral {:.6}. Available: {:.6}", trade_result.effective_cost, available_collateral) }, state).await;
        return;
    }

    // --- Phase 3: Atomic State Updates ---
    let mut affected_user_ids = HashSet::new();
    affected_user_ids.insert(trader_user_id.to_string());

    let final_price;
    let final_supply = trade_result.final_supply;

    // --- Update Post --- 
    match state.posts.get_mut(&post_id) {
        Some(mut post_entry) => {
            println!("    - Updating Post {}: Initial Supply = {:.6}, Calculated Final Supply = {:.6}", post_id, post_entry.supply, final_supply);
            let supply_before_update = post_entry.supply; // Store pre-update value for logging
            post_entry.supply = final_supply;
            final_price = get_price(final_supply);
            post_entry.price = Some(final_price);
            println!("    - Post {} updated: Supply Before = {:.6}, Supply After = {:.6}, Final Price = {:.6}", post_id, supply_before_update, post_entry.supply, final_price);
        },
        None => { eprintln!("Critical Error: Post {} disappeared during trade processing.", post_id); return; }
    }

    // --- Update Trader State --- 
    let trader_rpnl_change = -trade_result.effective_cost; 
    println!("handle_buy: Updating trader state...");
    { // Scope for user_positions access
        let mut trader_pos_map = state.user_positions.entry(trader_user_id.to_string()).or_default();
        let mut trader_pos = trader_pos_map.entry(post_id).or_default();

        trader_pos.size += quantity;
        trader_pos.total_cost_basis += trade_result.effective_cost; // Simplification
        println!("handle_buy: Updated trader position: Size={:.4}, Basis={:.4}", trader_pos.size, trader_pos.total_cost_basis);

        if trader_pos.size.abs() < EPSILON {
            trader_pos.size = 0.0;
            trader_pos.total_cost_basis = 0.0;
            println!("handle_buy: Trader position size near zero, reset basis.");
        }
    } // Locks on user_positions released here
    println!("handle_buy: Finished user_positions update scope.");

    { // Scope for user_realized_pnl access
         println!("handle_buy: Updating user_realized_pnl...");
        state.user_realized_pnl.entry(trader_user_id.to_string())
            .and_modify(|rpnl| *rpnl += trader_rpnl_change)
            .or_insert(trader_rpnl_change);
         println!("handle_buy: user_realized_pnl updated by {:.4}.", trader_rpnl_change);
    } // Lock on user_realized_pnl released here
     println!("handle_buy: Finished user_realized_pnl update scope.");

    // Update Trader Exposure 
    let new_total_exposure = calculate_total_exposure(trader_user_id, state);
    state.user_exposure.insert(trader_user_id.to_string(), new_total_exposure);
    println!("handle_buy: Updated trader exposure to {:.4}.", new_total_exposure);

    println!("handle_buy: Updating liquidated users (if any)...");
    // --- Update Liquidated Users --- 
    for (liquidated_user_id, forced_trade_pnl) in &trade_result.liquidated_users {
        println!("   - Processing state update for liquidated user: {}", liquidated_user_id);
        affected_user_ids.insert(liquidated_user_id.clone());
        let mut liq_pos_removed = false;

        if let Some(mut liq_pos_map) = state.user_positions.get_mut(liquidated_user_id) {
             if liq_pos_map.remove(&post_id).is_some() {
                 liq_pos_removed = true;
                 println!("     - Removed position for post {}", post_id);
             } else {
                 println!("     - Warning: Position for post {} not found for liquidated user {}.", post_id, liquidated_user_id);
             }
        } else {
            println!("     - Warning: Position map not found for liquidated user {}.", liquidated_user_id);
        }

        if liq_pos_removed { // Only update PnL if position was confirmed removed
            state.user_realized_pnl.entry(liquidated_user_id.clone())
                .and_modify(|rpnl| *rpnl += forced_trade_pnl)
                .or_insert(*forced_trade_pnl);
            println!("     - Updated RPnL by {:.4}", forced_trade_pnl);
        } 
        
        // Reset exposure (simplistic)
        state.user_exposure.entry(liquidated_user_id.clone()).and_modify(|exp| *exp = 0.0).or_insert(0.0);
        println!("     - Reset exposure for user {}", liquidated_user_id);
    }
    println!("handle_buy: Finished updating liquidated users.");

    // --- Phase 4: Post-Trade Updates & Broadcasts ---
    // Thresholds update is now called from handle_client_message AFTER the handler returns

    // Log Results
            println!(
        "-> Buy OK (Qty: {:.6}, EffCost: {:.6}): Post {} -> Supply: {:.6}, Prc: {:.6}. Liqs: {}",
        quantity, trade_result.effective_cost, post_id, final_supply, final_price, trade_result.liquidated_users.len()
    );

    // Broadcast Market Updates 
    println!("handle_buy: Broadcasting market updates...");
    broadcast_market_and_position_updates(post_id, final_price, final_supply, client_id, state).await;
    println!("handle_buy: Returned from market update broadcast.");

    // Send UserSync Updates to all affected users
    println!("handle_buy: Preparing UserSync client map...");
    let client_map: HashMap<String, Uuid> = state.clients.iter()
        .map(|entry| (entry.value().user_id.clone(), *entry.key()))
        .collect();
    println!("handle_buy: Client map created (size: {}). Sending UserSync updates...", client_map.len());

    for user_id in affected_user_ids {
        if let Some(affected_client_id) = client_map.get(&user_id) {
            println!("   - Sending UserSync update to affected User {} (Client {})", user_id, affected_client_id);
            let state_clone = state.clone(); 
            let user_id_clone = user_id.clone();
            let affected_client_id_clone = *affected_client_id;
            // Add a small delay before accessing state again in send_user_sync_update
            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
            send_user_sync_update(&user_id_clone, affected_client_id_clone, &state_clone).await;
             println!("   - Returned from send_user_sync_update for User {} (Client {})", user_id, affected_client_id);
        } else {
            println!("   - Skipping UserSync update for User {} (offline?)", user_id);
        }
    }
    println!("handle_buy: Finished sending UserSync updates loop. Returning from handle_buy normally.");
}

async fn handle_sell(
    client_id: Uuid,
    trader_user_id: &str,
    post_id: Uuid,
    quantity: f64,
    state: &AppState,
) {
    if quantity <= EPSILON { send_to_client(client_id, ServerMessage::Error { message: "Sell quantity must be positive".to_string() }, state).await; return; }
    let trade_quantity = -quantity; // Internal representation
    ensure_user_state_exists(trader_user_id, state);

    // --- Phase 1: Read Initial State & Calculate Effective Trade ---
    let initial_supply = match state.posts.get(&post_id) { 
        Some(post_entry) => post_entry.supply, 
        None => { send_to_client(client_id, ServerMessage::Error { message: format!("Post {} not found", post_id) }, state).await; return; }
    };
    let trade_result = match calculate_effective_cost_and_final_supply(initial_supply, trade_quantity, post_id, state) { 
        Ok(result) => result, 
        Err(e) => { send_to_client(client_id, ServerMessage::Error { message: format!("Trade calculation error: {}", e) }, state).await; return; }
    };

    // --- Phase 2: Collateral & Position Checks ---
    let balance = state.user_balances.get(trader_user_id).map_or(INITIAL_BALANCE, |v| *v.value());
    let realized_pnl = state.user_realized_pnl.get(trader_user_id).map_or(0.0, |v| *v.value());
    let available_collateral = balance + realized_pnl;

    if trade_result.effective_cost > available_collateral + EPSILON { 
        send_to_client(client_id, ServerMessage::Error { message: format!("Insufficient collateral {:.6}. Available: {:.6}", trade_result.effective_cost, available_collateral) }, state).await; 
        return;
    }

    // --- Phase 3: Atomic State Updates (similar scoping as handle_buy) --- 
    let mut affected_user_ids = HashSet::new();
    affected_user_ids.insert(trader_user_id.to_string());
    let final_price;
    let final_supply = trade_result.final_supply;

    // Update Post (scope implicitly handled by get_mut)
    match state.posts.get_mut(&post_id) { 
         Some(mut post_entry) => {
            println!("    - Updating Post {}: Initial Supply = {:.6}, Calculated Final Supply = {:.6}", post_id, post_entry.supply, final_supply);
            let supply_before_update = post_entry.supply; 
            post_entry.supply = final_supply;
            final_price = get_price(final_supply);
            post_entry.price = Some(final_price);
            println!("    - Post {} updated: Supply Before = {:.6}, Supply After = {:.6}, Final Price = {:.6}", post_id, supply_before_update, post_entry.supply, final_price);
        },
        None => { eprintln!("Critical Error: Post {} disappeared during trade processing.", post_id); return; }
    }; 

    // Update Trader State with scopes
    let trader_rpnl_change = -trade_result.effective_cost; // Proceeds = -Cost
    println!("handle_sell: Updating trader state...");
    {
        let mut trader_pos_map = state.user_positions.entry(trader_user_id.to_string()).or_default();
        let mut trader_pos = trader_pos_map.entry(post_id).or_default();
        let old_size = trader_pos.size;
        trader_pos.size += trade_quantity; // trade_quantity is negative for sell
        trader_pos.total_cost_basis += trade_result.effective_cost;
        println!("handle_sell: Updated trader position: OldSize={:.4}, NewSize={:.4}, BasisChange={:.4}", old_size, trader_pos.size, trade_result.effective_cost);

        // Check if position closed and reset basis
        if trader_pos.size.abs() < EPSILON {
            println!("handle_sell: Trader position size near zero ({:.6}), resetting basis.", trader_pos.size);
            trader_pos.size = 0.0; 
            trader_pos.total_cost_basis = 0.0; 
        }
    }
    println!("handle_sell: Finished user_positions update scope.");

    {
         println!("handle_sell: Updating user_realized_pnl...");
        state.user_realized_pnl.entry(trader_user_id.to_string()) 
            .and_modify(|rpnl| *rpnl += trader_rpnl_change)
            .or_insert(trader_rpnl_change);
        println!("handle_sell: user_realized_pnl updated by {:.4}.", trader_rpnl_change);
    }
    println!("handle_sell: Finished user_realized_pnl update scope.");

    // Update Trader Exposure
    let new_total_exposure = calculate_total_exposure(trader_user_id, state);
    state.user_exposure.insert(trader_user_id.to_string(), new_total_exposure);
    println!("handle_sell: Updated trader exposure to {:.4}.", new_total_exposure);

    println!("handle_sell: Updating liquidated users (if any)...");
    for (liquidated_user_id, forced_trade_pnl) in &trade_result.liquidated_users {
        affected_user_ids.insert(liquidated_user_id.clone());
        let mut liq_pos_removed = false;
        if let Some(mut liq_pos_map) = state.user_positions.get_mut(liquidated_user_id) {
             if liq_pos_map.remove(&post_id).is_some() { liq_pos_removed = true; println!("     - Removed liq position for post {}", post_id); } 
        } 
        if liq_pos_removed { 
            state.user_realized_pnl.entry(liquidated_user_id.clone())
                .and_modify(|rpnl| *rpnl += forced_trade_pnl)
                .or_insert(*forced_trade_pnl); 
            println!("     - Updated liq RPnL by {:.4}", forced_trade_pnl);
        } 
        state.user_exposure.entry(liquidated_user_id.clone()).and_modify(|exp| *exp = 0.0).or_insert(0.0);
        println!("     - Reset liq exposure for user {}", liquidated_user_id);
    }
    println!("handle_sell: Finished updating liquidated users.");

    // --- Phase 4: Post-Trade Updates & Broadcasts --- 
    println!(
        "-> Sell OK (Qty: {:.6}, EffProceeds: {:.6}): Post {} -> Supply: {:.6}, Prc: {:.6}. Liqs: {}",
        quantity, -trade_result.effective_cost, 
        post_id, final_supply, final_price, trade_result.liquidated_users.len()
    );

    println!("handle_sell: Broadcasting market updates...");
    broadcast_market_and_position_updates(post_id, final_price, final_supply, client_id, state).await;
    println!("handle_sell: Returned from market update broadcast.");

    // Send UserSync Updates
    println!("handle_sell: Preparing UserSync client map...");
    let client_map: HashMap<String, Uuid> = state.clients.iter()
        .map(|entry| (entry.value().user_id.clone(), *entry.key()))
        .collect();
    println!("handle_sell: Client map created (size: {}). Sending UserSync updates...", client_map.len());

    for user_id in affected_user_ids {
        if let Some(affected_client_id) = client_map.get(&user_id) {
            println!("   - Sending UserSync update to affected User {} (Client {})", user_id, affected_client_id);
            let state_clone = state.clone();
            let user_id_clone = user_id.clone();
            let affected_client_id_clone = *affected_client_id;
            // Add a small delay before accessing state again in send_user_sync_update
            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
            send_user_sync_update(&user_id_clone, affected_client_id_clone, &state_clone).await;
            println!("   - Returned from send_user_sync_update for User {} (Client {})", user_id, affected_client_id);
        } else {
             println!("   - Skipping UserSync update for User {} (offline?)", user_id);
        }
    }
    println!("handle_sell: Finished sending UserSync updates loop. Returning from handle_sell normally.");
}

// Function to recalculate and update liquidation thresholds for a post
async fn update_liquidation_thresholds(post_id: Uuid, state: &AppState) {
    let start_time = Instant::now();
    println!("--- Entering update_liquidation_thresholds for Post: {} ---", post_id);

    // Temporary map to store user-specific thresholds before aggregating
    // Key: s_liq (as OrderedFloat), Value: Vec<(cost_unwind, size_unwind, user_id)>
    let mut aggregated_thresholds: BTreeMap<OrderedFloat<f64>, Vec<(f64, f64, String)>> = BTreeMap::new();

    println!("update_liquidation_thresholds: Starting Phase 1 - Iterating user positions...");
    // --- Phase 1: Calculate individual user liquidation points & data ---
    for user_entry in state.user_positions.iter() {
        let user_id = user_entry.key();
        println!("update_liquidation_thresholds: Checking user: {}", user_id);
        if let Some(position) = user_entry.value().get(&post_id) {
            println!("update_liquidation_thresholds: Found position for user {} on post {}: Size={:.4}", user_id, post_id, position.size);
            if position.size.abs() < EPSILON { continue; }

            println!("update_liquidation_thresholds: Calculating for user {}: Getting balance/rpnl...", user_id);
            let balance = state.user_balances.get(user_id).map_or(0.0, |v| *v.value());
            let rpnl = state.user_realized_pnl.get(user_id).map_or(0.0, |v| *v.value());
            println!("update_liquidation_thresholds: User {}: Bal={:.4}, RPnl={:.4}. Getting market price...", user_id, balance, rpnl);
            let current_market_price = state.posts.get(&post_id)
                .map_or(0.0, |p| p.value().price.unwrap_or(0.0)); // Fixed: Use price, unwrap Option
            println!("update_liquidation_thresholds: User {}: MarketPrice={:.4}. Calculating avg_price...", user_id, current_market_price);

            let avg_price = calculate_average_price(&position);
            println!("update_liquidation_thresholds: User {}: AvgPrice={:.4}. Calculating uRPnL...", user_id, avg_price);
            let total_unrealized_pnl = (current_market_price - avg_price) * position.size;
            println!("update_liquidation_thresholds: User {}: uRPnL={:.4}. Calculating liquidation supply...", user_id, total_unrealized_pnl);

            if let Some(s_liq) = calculate_liquidation_supply(balance, rpnl, position.size, avg_price) {
                println!("update_liquidation_thresholds: User {}: Calculated s_liq = {:.4}. Calculating unwind...", user_id, s_liq);
                let forced_trade_size = -position.size;
                let s_liq_after_unwind = s_liq + forced_trade_size;
                let cost_unwind = calculate_smooth_cost(s_liq, s_liq_after_unwind);
                println!("update_liquidation_thresholds: User {}: ForcedSize={:.4}, s_liq_after={:.4}, CostUnwind={:.4}. Adding to map...", user_id, forced_trade_size, s_liq_after_unwind, cost_unwind);

                // Add this user's liquidation data to the aggregation map
                let supply_key = OrderedFloat(s_liq);
                aggregated_thresholds.entry(supply_key)
                    .or_default() // Get Vec or create new one
                    .push((cost_unwind, forced_trade_size, user_id.clone()));
                println!("update_liquidation_thresholds: User {}: Added entry for s_liq = {:.4}.", user_id, s_liq);
            } else {
                println!("update_liquidation_thresholds: User {}: No liquidation supply calculated.", user_id);
            }
         } else {
            println!("update_liquidation_thresholds: User {} has no position on post {}.", user_id, post_id);
        }
    }
    println!("update_liquidation_thresholds: Finished Phase 1. Starting Phase 2 - Updating state...");

    // --- Phase 2: Update state --- 
    // Remove thresholds where the net effect is negligible (optional optimization)
     aggregated_thresholds.retain(|_, entries| {
         entries.iter().any(|(cost, size, _)| cost.abs() > EPSILON || size.abs() > EPSILON)
     });
    println!("update_liquidation_thresholds: Retained {} aggregated thresholds.", aggregated_thresholds.len());

    // Insert the newly calculated map into the shared state
    state.liquidation_thresholds.insert(post_id, aggregated_thresholds);
    println!("update_liquidation_thresholds: Inserted aggregated map into state.liquidation_thresholds.");

    let duration = start_time.elapsed();
     println!("--- Finished Updating Liquidation Thresholds for Post: {}. Took {:?}. Entries: {} ---", post_id, duration, state.liquidation_thresholds.get(&post_id).map_or(0, |m| m.len()));
} 


