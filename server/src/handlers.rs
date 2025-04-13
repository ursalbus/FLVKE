use chrono::Utc;
use uuid::Uuid;
use warp::filters::ws::Message;

use super::state::AppState;
use super::models::{ClientMessage, ServerMessage, Post, UserPositionDetail};
use super::constants::{EPSILON, INITIAL_BALANCE};
use super::bonding_curve::{get_price, calculate_cost};
use super::calculations::{calculate_average_price, calculate_unrealized_pnl, calculate_user_margin};
use super::websocket::{send_to_client, broadcast_message, broadcast_market_and_position_updates};

pub async fn handle_client_message(
    client_id: Uuid,
    user_id: &str,
    msg: Message,
    state: &AppState,
) {
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
                    }
                     ClientMessage::Sell { post_id, quantity } => {
                        handle_sell(client_id, user_id, post_id, quantity, state).await;
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

    // --- Read needed state (minimal locking) ---
    let maybe_post_data = state.posts.get(&post_id).map(|p| (p.supply, p.price));
    let user_balance_val = state.user_balances.get(user_id).map_or(INITIAL_BALANCE, |b| *b.value());
    let user_position_val = state.user_positions.get(user_id)
        .and_then(|positions| positions.get(&post_id).map(|pos_detail| pos_detail.clone())); // Clone to release lock

    // --- Perform checks and calculations --- 
    let (current_supply, _) = match maybe_post_data {
        Some((supply, _)) => (supply, ()),
        None => {
            println!("-> Buy FAIL: Post {} not found", post_id);
            send_to_client(client_id, ServerMessage::Error { message: format!("Post {} not found", post_id) }, state).await;
            return;
        }
    };

    let new_supply = current_supply + quantity;
    let trade_cost = calculate_cost(current_supply, new_supply);

    if trade_cost.is_nan() {
        println!("-> Buy FAIL: Cost calculation resulted in NaN (Supplies: {} -> {})", current_supply, new_supply);
        send_to_client(client_id, ServerMessage::Error { message: "Internal error calculating trade cost.".to_string() }, state).await;
        return;
    }

    if user_balance_val < trade_cost {
         println!(
            "-> Buy FAIL: User {} Insufficient balance ({:.6}) for cost {:.6} (Quantity: {:.6})",
            user_id, user_balance_val, trade_cost, quantity
        );
        send_to_client(client_id, ServerMessage::Error { message: format!("Insufficient balance ({:.6}) for buy cost {:.6}", user_balance_val, trade_cost) }, state).await;
        return;
    }

    let old_position = user_position_val.unwrap_or_default();
    let old_size = old_position.size;
    let mut realized_pnl_for_trade = 0.0;
    let mut basis_change_for_short_cover = 0.0;
    let mut avg_short_entry_price = 0.0;
    let mut avg_cost_of_buy_reduction = 0.0;

    if old_size < -EPSILON {
        let reduction_amount = quantity.min(old_size.abs());
        if reduction_amount > EPSILON {
            avg_short_entry_price = if old_size.abs() > EPSILON { old_position.total_cost_basis / old_size } else { 0.0 };
            let exact_cost_for_reduction = calculate_cost(current_supply, current_supply + reduction_amount);
            avg_cost_of_buy_reduction = if reduction_amount.abs() > EPSILON { exact_cost_for_reduction / reduction_amount } else { 0.0 };
            realized_pnl_for_trade = (avg_short_entry_price - avg_cost_of_buy_reduction) * reduction_amount;
            basis_change_for_short_cover = avg_short_entry_price * reduction_amount;
        }
    }

    // --- Acquire locks and update state --- 
    let mut total_pnl_updated = false;
    let final_price;
    let final_position;
    let final_balance;
    let final_total_realized_pnl;

    { // Scope for mutable references
        let mut post_entry = match state.posts.get_mut(&post_id) {
             Some(entry) => entry,
             None => return,
         };
         let post = &mut *post_entry;
         post.supply = new_supply;
         final_price = get_price(new_supply);
         post.price = Some(final_price);

        let mut balance_entry = state.user_balances.entry(user_id.to_string()).or_insert(INITIAL_BALANCE);
        *balance_entry -= trade_cost;
        final_balance = *balance_entry;

        let user_positions_for_user = state.user_positions.entry(user_id.to_string()).or_default();
        let mut user_position = user_positions_for_user.entry(post_id).or_insert_with(UserPositionDetail::default);
        user_position.size += quantity;

        if old_size < -EPSILON {
             user_position.total_cost_basis += basis_change_for_short_cover;
              println!(
                "   -> Covering short: Reduction: {:.6}, Avg Short Entry Prc: {:.6}, Avg Buy Cost (Reduction Only): {:.6}, Basis Change: +{:.6}, Pnl: {:.6}",
                quantity.min(old_size.abs()), avg_short_entry_price, avg_cost_of_buy_reduction, basis_change_for_short_cover, realized_pnl_for_trade
              );
         }

        if old_size >= -EPSILON { // Started flat or long
            user_position.total_cost_basis += trade_cost;
            println!("   -> Adding Long Basis (Flat/Long Start): Cost: {:.6}, Total Basis: {:.6}",
                    trade_cost, user_position.total_cost_basis);
        } else if quantity > old_size.abs() { // Covered short AND opened long
            let long_opening_quantity = quantity - old_size.abs();
            let supply_at_zero_crossing = current_supply + old_size.abs();
            let cost_for_long_part = calculate_cost(supply_at_zero_crossing, new_supply);
            user_position.total_cost_basis += cost_for_long_part;
            println!("   -> Adding Long Basis (Short Cover + Open Long): Qty: {:.6}, Cost for Long Part (Supply {:.6} -> {:.6}): {:.6}, Total Basis: {:.6}",
                    long_opening_quantity, supply_at_zero_crossing, new_supply, cost_for_long_part, user_position.total_cost_basis);
        }

        if realized_pnl_for_trade.abs() > EPSILON {
            println!(
                "   -> Realizing PNL (Buy Cover): {:.6} for User {}",
                realized_pnl_for_trade, user_id
            );
            let mut total_pnl = state.user_realized_pnl.entry(user_id.to_string()).or_insert(0.0);
            *total_pnl += realized_pnl_for_trade;
            final_total_realized_pnl = *total_pnl;
            total_pnl_updated = true;
        } else {
            final_total_realized_pnl = state.user_realized_pnl.get(user_id).map_or(0.0, |v| *v.value());
        }

         if user_position.size.abs() < EPSILON {
             println!("   -> Position size is near zero ({:.8}) after trade, resetting basis.", user_position.size);
             user_position.size = 0.0;
             user_position.total_cost_basis = 0.0;
         }

        final_position = user_position.clone();

    } // Mutable references are dropped here

    // --- Log results and send updates --- 
    let display_avg_price = calculate_average_price(&final_position);
    let unrealized_pnl = calculate_unrealized_pnl(&final_position, final_price);

     println!(
        "-> Buy OK (Qty: {:.6}, Cost: {:.6}): Post {} (Supply: {:.6}, Price: {:.6}), User {} (Pos: {:.6}, Avg Prc: {:.6}, URPnl: {:.6}, Bal: {:.6}) RPnlTrade: {:.6}",
        quantity, trade_cost, post_id, new_supply, final_price, user_id, final_position.size,
        display_avg_price.abs(), unrealized_pnl, final_balance, realized_pnl_for_trade
    );

     send_to_client(client_id, ServerMessage::BalanceUpdate { balance: final_balance }, state).await;
    if total_pnl_updated {
         send_to_client(client_id, ServerMessage::RealizedPnlUpdate { total_realized_pnl: final_total_realized_pnl }, state).await;
    }
    send_to_client(client_id, ServerMessage::PositionUpdate {
        post_id, size: final_position.size,
        average_price: display_avg_price.abs(),
        unrealized_pnl
     }, state).await;
    let new_margin = calculate_user_margin(user_id, state);
    send_to_client(client_id, ServerMessage::MarginUpdate { margin: new_margin }, state).await;
    broadcast_market_and_position_updates(post_id, final_price, new_supply, client_id, state).await;
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

    // --- Read needed state (minimal locking) ---
    let maybe_post_data = state.posts.get(&post_id).map(|p| (p.supply, p.price));
    let user_balance_val = state.user_balances.get(user_id).map_or(INITIAL_BALANCE, |b| *b.value());
    let user_position_val = state.user_positions.get(user_id)
        .and_then(|positions| positions.get(&post_id).map(|pos_detail| pos_detail.clone()));

    // --- Perform checks and calculations --- 
    let (current_supply, _) = match maybe_post_data {
        Some((supply, _)) => (supply, ()),
        None => {
            println!("-> Sell FAIL: Post {} not found", post_id);
            send_to_client(client_id, ServerMessage::Error { message: format!("Post {} not found", post_id) }, state).await;
            return;
        }
    };

    let new_supply = current_supply - quantity;
    let trade_proceeds = calculate_cost(new_supply, current_supply);

    if trade_proceeds.is_nan() || trade_proceeds < 0.0 {
        println!(
            "-> Sell FAIL: Proceeds calculation invalid (Supplies: {} -> {}, Proceeds: {})",
            current_supply, new_supply, trade_proceeds
        );
        send_to_client(client_id, ServerMessage::Error { message: "Internal error calculating trade proceeds.".to_string() }, state).await;
        return;
    }

    let old_position = user_position_val.unwrap_or_default();
    let old_size = old_position.size;

    if old_size <= EPSILON && quantity > EPSILON { // User is flat or already short, and selling more
        let cost_to_open_or_increase_short = trade_proceeds;
        if user_balance_val < cost_to_open_or_increase_short {
            println!(
                "-> Sell FAIL (Short): User {} Insufficient balance ({:.6}) for collateral {:.6} (Quantity: {:.6})",
                user_id, user_balance_val, cost_to_open_or_increase_short, quantity
            );
            send_to_client(client_id, ServerMessage::Error { message: format!("Insufficient balance ({:.6}) to cover short collateral {:.6}", user_balance_val, cost_to_open_or_increase_short) }, state).await;
            return;
        }
    }

    let mut realized_pnl_for_trade = 0.0;
    let mut basis_removed = 0.0;
    let mut avg_price_before_long_close = 0.0;
    let mut avg_proceeds_of_sell_reduction = 0.0;

    if old_size > EPSILON {
        let reduction_amount = quantity.min(old_size);
        if reduction_amount > EPSILON {
            avg_price_before_long_close = calculate_average_price(&old_position);
            let exact_proceeds_for_reduction = calculate_cost(current_supply - reduction_amount, current_supply);
            avg_proceeds_of_sell_reduction = if reduction_amount.abs() > EPSILON { exact_proceeds_for_reduction / reduction_amount } else { 0.0 };
            realized_pnl_for_trade = (avg_proceeds_of_sell_reduction - avg_price_before_long_close) * reduction_amount;
            basis_removed = avg_price_before_long_close * reduction_amount;
        }
    }

    // --- Acquire locks and update state --- 
    let mut total_pnl_updated = false;
    let final_price;
    let final_position;
    let final_balance;
    let final_total_realized_pnl;

    { // Scope for mutable references
        let mut post_entry = match state.posts.get_mut(&post_id) {
             Some(entry) => entry,
             None => return,
         };
         let post = &mut *post_entry;
         post.supply = new_supply;
         final_price = get_price(new_supply);
         post.price = Some(final_price);

        let mut balance_entry = state.user_balances.entry(user_id.to_string()).or_insert(INITIAL_BALANCE);
        *balance_entry += trade_proceeds;
        final_balance = *balance_entry;

        let user_positions_for_user = state.user_positions.entry(user_id.to_string()).or_default();
        let mut user_position = user_positions_for_user.entry(post_id).or_insert_with(UserPositionDetail::default);
        user_position.size -= quantity;

         if old_size > EPSILON {
             user_position.total_cost_basis -= basis_removed;
             println!("   -> Selling long: Reduction: {:.6}, Avg Buy Prc: {:.6}, Avg Sell Prc (Reduction Only): {:.6}, Basis Removed: {:.6}, Pnl: {:.6}",
                quantity.min(old_size), avg_price_before_long_close, avg_proceeds_of_sell_reduction, basis_removed, realized_pnl_for_trade);
         }

        if old_size <= EPSILON { // Started flat or short
            let shorting_quantity = quantity;
            let proceeds_for_shorting_part = calculate_cost(new_supply, new_supply + shorting_quantity); 
            user_position.total_cost_basis -= proceeds_for_shorting_part;
            println!("   -> Opening/Increasing Short: Qty: {:.6}, Proceeds for Short Part: {:.6}, Basis Change: {:.6}",
                    shorting_quantity, proceeds_for_shorting_part, -proceeds_for_shorting_part);
        } else if quantity > old_size { // Closed long AND opened short
            let shorting_quantity = quantity - old_size;
            let supply_at_zero_crossing = current_supply - old_size;
            let proceeds_for_shorting_part = calculate_cost(new_supply, supply_at_zero_crossing);
            user_position.total_cost_basis -= proceeds_for_shorting_part;
            println!("   -> Opened Short (from Long): Qty: {:.6}, Proceeds for Short Part (Supply {:.6} -> {:.6}): {:.6}, Basis Change: {:.6}",
                    shorting_quantity, supply_at_zero_crossing, new_supply, proceeds_for_shorting_part, -proceeds_for_shorting_part);
        }

        if realized_pnl_for_trade.abs() > EPSILON {
            println!(
                "   -> Realizing PNL (Sell Long): {:.6} for User {}",
                realized_pnl_for_trade, user_id
            );
            let mut total_pnl = state.user_realized_pnl.entry(user_id.to_string()).or_insert(0.0);
            *total_pnl += realized_pnl_for_trade;
            final_total_realized_pnl = *total_pnl;
            total_pnl_updated = true;
         } else {
            final_total_realized_pnl = state.user_realized_pnl.get(user_id).map_or(0.0, |v| *v.value());
        }

         if user_position.size.abs() < EPSILON {
             println!("   -> Position size is near zero ({:.8}) after trade, resetting basis.", user_position.size);
             user_position.size = 0.0;
             user_position.total_cost_basis = 0.0;
         }

        final_position = user_position.clone();
    } // Mutable references are dropped here

    // --- Log results and send updates --- 
    let display_avg_price = calculate_average_price(&final_position);
    let unrealized_pnl = calculate_unrealized_pnl(&final_position, final_price);

     println!(
        "-> Sell OK (Qty: {:.6}, Proceeds: {:.6}): Post {} (Supply: {:.6}, Price: {:.6}), User {} (Pos: {:.6}, Avg Prc: {:.6}, URPnl: {:.6}, Bal: {:.6}) RPnlTrade: {:.6}",
        quantity, trade_proceeds,
        post_id, new_supply, final_price, user_id, final_position.size,
        display_avg_price.abs(), unrealized_pnl, final_balance, realized_pnl_for_trade
    );

    send_to_client(client_id, ServerMessage::BalanceUpdate { balance: final_balance }, state).await;
    if total_pnl_updated {
         send_to_client(client_id, ServerMessage::RealizedPnlUpdate { total_realized_pnl: final_total_realized_pnl }, state).await;
    }
     send_to_client(client_id, ServerMessage::PositionUpdate {
            post_id, size: final_position.size,
            average_price: display_avg_price.abs(),
            unrealized_pnl
         }, state).await;
    let new_margin = calculate_user_margin(user_id, state);
    send_to_client(client_id, ServerMessage::MarginUpdate { margin: new_margin }, state).await;
    broadcast_market_and_position_updates(post_id, final_price, new_supply, client_id, state).await;
} 