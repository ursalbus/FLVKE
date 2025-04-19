use futures_util::{StreamExt, SinkExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;
use warp::filters::ws::{Message, WebSocket};

use super::state::AppState;
use super::models::{Client, ServerMessage, PositionDetail};
use super::constants::{INITIAL_BALANCE, EPSILON};
use super::bonding_curve::get_price;
use super::calculations::{calculate_average_price, calculate_unrealized_pnl, calculate_liquidation_price};
use super::handlers::{calculate_total_unrealized_pnl, handle_client_message, send_user_sync_update};

// --- WebSocket Handling ---

// Helper to get simple message type string for logging
pub fn message_type_for_debug(msg: &ServerMessage) -> &'static str {
    match msg {
       ServerMessage::InitialState { .. } => "InitialState",
       ServerMessage::UserSync { .. } => "UserSync",
       ServerMessage::NewPost { .. } => "NewPost",
       ServerMessage::MarketUpdate { .. } => "MarketUpdate",
       ServerMessage::BalanceUpdate { .. } => "BalanceUpdate",
       ServerMessage::PositionUpdate { .. } => "PositionUpdate",
       ServerMessage::RealizedPnlUpdate { .. } => "RealizedPnlUpdate",
       ServerMessage::ExposureUpdate { .. } => "ExposureUpdate",
       ServerMessage::EquityUpdate { .. } => "EquityUpdate",
       ServerMessage::Error { .. } => "Error",
   }
}

// Helper to send a message to a specific client
pub async fn send_to_client(client_id: Uuid, message: ServerMessage, state: &AppState) {
    if let Some(client) = state.clients.get(&client_id) {
        match serde_json::to_string(&message) {
            Ok(json_msg) => {
                if client.sender.send(Ok(Message::text(json_msg))).is_err() {
                    eprintln!(
                        "Error queueing message type '{}' for client_id={}",
                        message_type_for_debug(&message),
                        client_id
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "Failed to serialize direct message '{}' for client_id={}: {}",
                     message_type_for_debug(&message),
                     client_id, e
                );
            }
        }
    } else {
         eprintln!(
            "Attempted to send direct message '{}' to non-existent client_id={}",
            message_type_for_debug(&message),
            client_id
        );
    }
}

// Original broadcast function (used for NewPost and MarketUpdate inside broadcast_market_and_position_updates)
pub async fn broadcast_message(message: ServerMessage, state: &AppState) {
     if state.clients.is_empty() {
        println!("No clients connected, skipping broadcast.");
        return;
    }
    let serialized_message = match serde_json::to_string(&message) {
        Ok(s) => s,
        Err(e) => {
             eprintln!("Failed to serialize broadcast message: {:?}, error: {}", message, e);
            return;
        }
    };
    println!(
        "Broadcasting message type: {} to {} clients",
        message_type_for_debug(&message),
        state.clients.len()
    );
    for client_entry in state.clients.iter() {
        let client_id = client_entry.key();
        let client = client_entry.value();
        if client.sender.send(Ok(Message::text(serialized_message.clone()))).is_err() {
            eprintln!("Failed to send broadcast message to client_id={}, user_id={}. Channel likely closed.", client_id, client.user_id);
        }
    }
}

// Function to broadcast market update and then send individual position/margin updates to OTHER clients
pub async fn broadcast_market_and_position_updates(
    post_id: Uuid,
    new_price: f64,
    new_supply: f64,
    trading_client_id: Uuid, // ID of the client who made the trade
    state: &AppState,
) {
    // 1. Broadcast the general market update to everyone
    println!("broadcast_market_and_position_updates: Broadcasting MarketUpdate...");
    let market_update_msg = ServerMessage::MarketUpdate {
        post_id,
        price: new_price,
        supply: new_supply,
    };
    broadcast_message(market_update_msg, state).await;
    println!("broadcast_market_and_position_updates: Finished MarketUpdate broadcast. Iterating clients for PnL/Equity...");

    // 2. Iterate through all ACTIVE clients to potentially send PNL and Equity updates
    for client_entry in state.clients.iter() {
        let current_client_id = *client_entry.key();
        let client_info = client_entry.value();
        let user_id = &client_info.user_id;

        println!("broadcast_market_and_position_updates: Checking client {} (User {})", current_client_id, user_id);
        // Skip the client who initiated this trade
        if current_client_id == trading_client_id {
             println!("broadcast_market_and_position_updates: Skipping trading client {}", current_client_id);
            continue;
        }

        let mut affected_by_price_change = false;

        // Check if this *other* user has a position in the updated post
         if let Some(user_positions_map) = state.user_positions.get(user_id) {
             if let Some(position) = user_positions_map.get(&post_id) {
                 println!("broadcast_market_and_position_updates: User {} has position in post {}. Size={:.4}", user_id, post_id, position.size);
                 // Only process if position exists and is non-zero (PnL change matters)
                 if position.size.abs() > EPSILON { 
                     println!("broadcast_market_and_position_updates: Position size non-zero. Calculating updates for user {}", user_id);
                     // We need to send the full UserSync to include the updated liq price
                     // (since PositionUpdate doesn't currently support it)
                     // The UserSync calculation automatically handles getting the latest data
                     println!(
                         "   -> Sending UserSync instead of PositionUpdate for Post {} to OTHER User {} ({}).",
                         post_id, user_id, current_client_id
                     );
                     // Call send_user_sync_update directly
                     send_user_sync_update(user_id, current_client_id, state).await;
                     println!("   -> Returned from send_user_sync_update call.");
                     affected_by_price_change = true; // Mark that PnL potentially changed
                 } else {
                     println!("broadcast_market_and_position_updates: Position size near zero for user {}, skipping PnL update.", user_id);
                 }
             } else {
                 println!("broadcast_market_and_position_updates: User {} has no position in post {}, skipping PnL update.", user_id, post_id);
             }
         } else {
              println!("broadcast_market_and_position_updates: User {} has no position map, skipping PnL update.", user_id);
         }

        // If the user's PNL for this post changed, their overall Equity also changed.
        // Send EquityUpdate. Exposure doesn't change from market moves, only trades.
        // This might be redundant if we send UserSync above, but let's keep it for now
        // as UserSync primarily updates the *target* user of the sync.
        if affected_by_price_change {
            println!("broadcast_market_and_position_updates: User {} was affected by price change, sending EquityUpdate.", user_id);
            // Recalculate total equity for this user
            let balance = state.user_balances.get(user_id).map_or(INITIAL_BALANCE, |b| *b.value());
            let realized_pnl = state.user_realized_pnl.get(user_id).map_or(0.0, |pnl| *pnl.value());
            let total_unrealized_pnl = calculate_total_unrealized_pnl(user_id, state);
            let equity = balance + realized_pnl + total_unrealized_pnl;

            println!(
                "   -> Sending Equity update to OTHER User {} ({}): {:.4}",
                 user_id, current_client_id, equity
            );
            send_to_client(current_client_id, ServerMessage::EquityUpdate { equity }, state).await;
        } else {
             println!("broadcast_market_and_position_updates: User {} not affected by price change, skipping EquityUpdate.", user_id);
        }
    }
     println!("broadcast_market_and_position_updates: Finished iterating clients.");
}

pub async fn handle_connection(ws: WebSocket, user_id: String, state: AppState) {
    let client_id = Uuid::new_v4();
    println!(
        "New WebSocket connection: client_id={}, user_id={}",
        client_id, &user_id
    );

    let (client_sender, client_rcv) = mpsc::unbounded_channel();
    let client_rcv_stream = UnboundedReceiverStream::new(client_rcv);

    state.user_balances.entry(user_id.clone()).or_insert(INITIAL_BALANCE);
    state.user_realized_pnl.entry(user_id.clone()).or_insert(0.0);
    state.user_exposure.entry(user_id.clone()).or_insert(0.0);

    state.clients.insert(
        client_id,
        Client {
            user_id: user_id.clone(),
            sender: client_sender.clone(),
        },
    );

    // --- Send InitialState (Global Posts) --- 
    let current_posts = state
        .posts
        .iter()
        .map(|entry| {
            let mut post = entry.value().clone();
            post.price = Some(get_price(post.supply)); // Ensure price is current
            post
        })
        .collect();
    let initial_state_msg = ServerMessage::InitialState { posts: current_posts };
    if client_sender.send(Ok(Message::text(serde_json::to_string(&initial_state_msg).unwrap()))).is_err() {
         eprintln!("Failed initial send (InitialState) to client_id={}", client_id);
         state.clients.remove(&client_id);
         return;
    }
    println!("Sent InitialState to client_id={}", client_id);

    // --- Send UserSync (Balance, Exposure, Equity, PnL, Positions) ---
    let user_balance = *state.user_balances.get(&user_id).unwrap().value();
    let total_realized_pnl = *state.user_realized_pnl.get(&user_id).unwrap().value();
    let user_exposure = *state.user_exposure.get(&user_id).unwrap().value();
    let total_unrealized_pnl = calculate_total_unrealized_pnl(&user_id, &state);
    let user_equity = user_balance + total_realized_pnl + total_unrealized_pnl;

    let user_positions_detail: Vec<PositionDetail> = state
        .user_positions
        .get(&user_id)
        .map(|positions_map| {
            positions_map
                .iter()
                .filter_map(|pos_entry| {
                    let post_id = *pos_entry.key();
                    let position = pos_entry.value();

                    if position.size.abs() <= EPSILON {
                         return None; 
                    }

                    state.posts.get(&post_id).map(|market_post| {
                        let current_market_price = market_post.price.unwrap_or_else(|| get_price(market_post.supply));
                        let avg_price = calculate_average_price(position);
                        let unrealized_pnl = calculate_unrealized_pnl(position, current_market_price);
                        
                        // Calculate liquidation price here too
                        let liquidation_price = calculate_liquidation_price(
                            user_balance, // Use fetched balance
                            total_realized_pnl, // Use fetched rpnl
                            position.size, 
                            avg_price
                        );

                        PositionDetail {
                            post_id,
                            size: position.size,
                            average_price: avg_price.abs(),
                            unrealized_pnl: unrealized_pnl,
                            liquidation_price, // Add field
                        }
                    })
                })
                .collect()
        })
        .unwrap_or_else(Vec::new);

    let user_sync_msg = ServerMessage::UserSync {
        balance: user_balance,
        exposure: user_exposure,
        equity: user_equity,
        positions: user_positions_detail,
        total_realized_pnl,
    };
     if client_sender.send(Ok(Message::text(serde_json::to_string(&user_sync_msg).unwrap()))).is_err() {
         eprintln!("Failed initial send (UserSync) to client_id={}", client_id);
         state.clients.remove(&client_id);
         return;
    }
     println!("Sent UserSync to client_id={} (Bal: {:.4}, RPnl: {:.4}, Exp: {:.4}, Equity: {:.4})",
        client_id, user_balance, total_realized_pnl, user_exposure, user_equity);

    // --- WebSocket Task Setup ---
    let (ws_sender, mut ws_receiver) = ws.split();

    // Task to forward messages from MPSC channel to WebSocket sink
    tokio::spawn(async move {
       let task_client_id = client_id;
       let mut ws_sender = ws_sender;
       let mut client_rcv_stream = client_rcv_stream;
       while let Some(message_result) = client_rcv_stream.next().await {
            match message_result {
                Ok(msg) => {
                    if ws_sender.send(msg).await.is_err() {
                        eprintln!(
                            "Error sending message via MPSC->WS forwarder task for client {}",
                            task_client_id
                        );
                        break; // Exit loop on send error
                    }
                }
                Err(e) => {
                    eprintln!(
                        "Error receiving message in MPSC->WS forwarder task for client {}: {}",
                        task_client_id, e
                    );
                    // Optionally break or continue depending on desired error handling
                }
            }
        }
       
        println!("MPSC->WS forwarder task finished for client {}", task_client_id);
    });

    // --- Main Message Loop ---
    while let Some(result) = ws_receiver.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!(
                    "WebSocket error receiving message for client_id={}: {}, user_id={}",
                    client_id, e, &user_id
                );
                break;
            }
        };

        println!(
            "handle_connection for client_id={}: Received msg: {:?}. Calling handle_client_message...",
            client_id, msg
        );
        handle_client_message(client_id, &user_id, msg, &state).await;
        println!(
            "handle_connection for client_id={}: Returned from handle_client_message.",
            client_id
        );
    }

    // --- Cleanup on Disconnect ---
    println!(
        "WebSocket connection closed for client_id={}, user_id={}",
        client_id, &user_id
    );
    state.clients.remove(&client_id);
} 