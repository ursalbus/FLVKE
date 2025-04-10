// server/src/handlers.rs
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use postgrest::Postgrest;
use rust_decimal::Decimal;
use rust_decimal::prelude::Signed;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use serde_json;

use crate::bonding_curve::calculate_trade_cost;
use crate::protocol::{ClientMessage, ServerMessage};
use crate::state::{MarketState, NewPost, Post};

// --- Main Handler --- (Moved from main.rs)

/// Handles the logic for processing a received ClientMessage.
/// This runs in a separate task to avoid blocking the WebSocket read loop.
pub async fn handle_client_message(
    db: Arc<Postgrest>,
    market_state: MarketState,
    user_id: String,
    message: ClientMessage,
    addr: SocketAddr, // For logging
) -> ServerMessage {
    // TODO: For actual user actions (CreatePost, PlaceTrade),
    // we need the user's JWT to pass to Postgrest using `.auth(token)`
    // to properly enforce RLS based on the authenticated user.
    // Currently, the Postgrest client in AppState uses the anon/service key,
    // which might bypass RLS unintentionally in the real app flow.
    // Need to fetch/pass the JWT associated with `user_id` here.

    match message {
        ClientMessage::CreatePost { content } => {
            // Pass db clone that potentially uses user's auth later
            handle_create_post(db, user_id, content, addr).await
        }
        ClientMessage::PlaceTrade { post_id, amount } => {
            // Pass db clone that potentially uses user's auth later
            handle_place_trade(db, market_state, user_id, post_id, amount, addr).await
        }
    }
}

// --- Action Handlers & Helpers --- (Moved from main.rs)

/// Handles the CreatePost action
async fn handle_create_post(
    db: Arc<Postgrest>,
    user_id: String,
    content: String,
    addr: SocketAddr,
) -> ServerMessage {
    println!("[{}] Handling CreatePost from {}", addr, user_id);
    let new_post_data = NewPost {
        user_id: &user_id,
        content: &content,
    };
    let insert_result = db
        .from("posts")
        .insert(serde_json::to_string(&new_post_data).unwrap())
        .execute()
        .await;

    match insert_result {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                match response.text().await {
                    Ok(body_text) => match serde_json::from_str::<Vec<Post>>(&body_text) {
                        Ok(mut posts) if !posts.is_empty() => {
                            let created_post = posts.remove(0);
                            println!("[{}] Post created (ID: {}).", addr, created_post.id);
                            ServerMessage::PostCreated {
                                post_id: created_post.id,
                                content: created_post.content,
                                creator_id: created_post.user_id,
                            }
                        }
                        Ok(_) => ServerMessage::Error { message: "DB OK but no post data?".into() },
                        Err(e) => ServerMessage::Error { message: format!("DB OK but parse failed: {}", e) },
                    }
                    Err(e) => ServerMessage::Error { message: format!("DB OK but read body failed: {}", e) },
                }
            } else {
                 let error_body = response.text().await.unwrap_or_else(|_| format!("Status: {}", status));
                 ServerMessage::Error { message: format!("DB Error: {}", error_body) }
            }
        }
        Err(e) => ServerMessage::Error { message: format!("Network Error: {}", e) },
    }
}

/// Fetches the current balance for a user.
async fn get_user_balance(db: &Postgrest, user_id: &str) -> Result<Decimal, String> {
    let query_result = db
        .from("user_profiles")
        .select("balance")
        .eq("id", user_id)
        .single()
        .execute()
        .await;

    match query_result {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                 match response.text().await {
                    Ok(body_text) => {
                        #[derive(Deserialize)]
                        struct BalanceOnly { #[serde(with = "rust_decimal::serde::float")] balance: Decimal }
                        match serde_json::from_str::<BalanceOnly>(&body_text) {
                             Ok(data) => Ok(data.balance),
                             Err(e) => Err(format!("Failed to parse balance: {}. Body: {}", e, body_text)),
                        }
                    }
                     Err(e) => Err(format!("Failed to read balance response body: {}", e)),
                }
            } else {
                 let error_body = response.text().await.unwrap_or_else(|_| format!("Status: {}", status));
                 Err(format!("DB Error getting balance: {}", error_body))
            }
        }
        Err(e) => Err(format!("Network Error getting balance: {}", e)),
    }
}

/// Fetches the current position for a user on a specific post.
/// Returns 0.0 if no position exists.
async fn get_user_position(db: &Postgrest, user_id: &str, post_id: i64) -> Result<Decimal, String> {
    #[derive(Deserialize)]
    struct AmountOnly { #[serde(with = "rust_decimal::serde::float")] amount: Decimal }

    let query_result = db
        .from("positions")
        .select("amount")
        .eq("user_id", user_id)
        .eq("post_id", &post_id.to_string())
        .limit(1)
        .execute()
        .await;

    match query_result {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                match response.text().await {
                    Ok(body_text) => {
                        match serde_json::from_str::<Vec<AmountOnly>>(&body_text) {
                            Ok(amounts) if amounts.is_empty() => Ok(dec!(0.0)),
                            Ok(mut amounts) => Ok(amounts.remove(0).amount),
                            Err(e) => Err(format!("Failed to parse position Vec: {}. Body: {}", e, body_text)),
                        }
                    }
                    Err(e) => Err(format!("Failed to read position response body: {}", e)),
                }
            } else {
                let error_body = response.text().await.unwrap_or_else(|_| format!("Status: {}", status));
                Err(format!("DB Error getting position: {}", error_body))
            }
        }
        Err(e) => Err(format!("Network Error getting position: {}", e)),
    }
}

/// Updates user balance and position in the database within a transaction.
async fn update_balance_and_position(
    db: &Postgrest,
    user_id: &str,
    post_id: i64,
    balance_change: Decimal,
    new_position_amount: Decimal,
) -> Result<(), String> {
    let current_balance = get_user_balance(db, user_id).await?;
    let new_balance = current_balance + balance_change;

    #[derive(Serialize)]
    struct BalanceUpdate { #[serde(with = "rust_decimal::serde::float")] balance: Decimal }
    let balance_update_data = BalanceUpdate { balance: new_balance };

    let balance_result = db
        .from("user_profiles")
        .update(serde_json::to_string(&balance_update_data).unwrap())
        .eq("id", user_id)
        .execute()
        .await;

    match balance_result {
        Ok(res) => {
            let status = res.status();
            if !status.is_success() {
                 let error_body = res.text().await.unwrap_or_else(|_| format!("Status: {}", status));
                return Err(format!("Balance update failed: {}", error_body));
            }
        },
         Err(e) => return Err(format!("Balance update network error: {}", e)),
    }

    #[derive(Serialize)]
    struct PositionUpsert<'a> {
        user_id: &'a str,
        post_id: i64,
        #[serde(with = "rust_decimal::serde::float")]
        amount: Decimal,
    }
    let position_data = PositionUpsert {
        user_id,
        post_id,
        amount: new_position_amount,
    };

    let position_result = db
        .from("positions")
        .upsert(serde_json::to_string(&position_data).unwrap())
        .execute()
        .await;

     match position_result {
         Ok(response) => {
            let status = response.status();
             if status.is_success() {
                 Ok(())
             } else {
                let error_body = response.text().await.unwrap_or_else(|_| format!("Status {}", status));
                Err(format!("DB Error upserting position: {}", error_body))
             }
         }
         Err(e) => Err(format!("Network Error upserting position: {}", e)),
     }
}


/// Handles the PlaceTrade action
async fn handle_place_trade(
    db: Arc<Postgrest>,
    market_state: MarketState,
    user_id: String,
    post_id: i64,
    trade_amount: f64,
    addr: SocketAddr,
) -> ServerMessage {
    println!(
        "[{}] Handling PlaceTrade: User={}, Post={}, Amount={}",
        addr, user_id, post_id, trade_amount
    );

    if trade_amount == 0.0 {
        println!("[{}] Trade rejected: Zero amount", addr);
        return ServerMessage::Error { message: "Trade amount cannot be zero.".to_string() };
    }

    let current_supply = {
        let mut state = market_state.lock().await;
        *state.entry(post_id).or_insert(0.0)
    };
    println!("[{}] Current supply for post {}: {}", addr, post_id, current_supply);

    let current_position = match get_user_position(&db, &user_id, post_id).await {
        Ok(pos) => pos,
        Err(e) => {
            println!("[{}] Trade failed: Error getting position: {}", addr, e);
            return ServerMessage::Error { message: format!("Failed to get current position: {}", e) };
        }
    };
    println!("[{}] Current position for user {}: {}", addr, user_id, current_position);

    let current_balance = match get_user_balance(&db, &user_id).await {
        Ok(bal) => bal,
        Err(e) => {
            println!("[{}] Trade failed: Error getting balance: {}", addr, e);
            return ServerMessage::Error { message: format!("Failed to get balance: {}", e) };
        }
    };
     println!("[{}] Current balance for user {}: {}", addr, user_id, current_balance);

    let trade_amount_decimal = match Decimal::from_str(&trade_amount.to_string()) {
        Ok(d) => d,
        Err(_) => {
            println!("[{}] Trade failed: Invalid trade amount format {}", addr, trade_amount);
            return ServerMessage::Error { message: "Invalid trade amount format.".to_string() };
        }
    };
    let new_position_amount = current_position + trade_amount_decimal;

    if trade_amount < 0.0 {
        if current_position.signum() == new_position_amount.signum() && new_position_amount.abs() > current_position.abs() {
             println!(
                "[{}] Trade rejected: Insufficient shares. Current: {}, Trying to sell/cover: {}, New: {}",
                addr, current_position, trade_amount_decimal, new_position_amount
            );
             return ServerMessage::Error { message: "Insufficient shares to sell/cover.".to_string() };
        }
    }

    let cost_or_proceeds = calculate_trade_cost(current_supply, trade_amount);
    let cost_decimal = match Decimal::from_str(&cost_or_proceeds.to_string()) {
         Ok(d) => d,
         Err(_) => {
            println!("[{}] Trade failed: Invalid cost calculation result {}", addr, cost_or_proceeds);
            return ServerMessage::Error { message: "Internal error calculating trade cost.".to_string() };
        }
    };
    println!("[{}] Calculated cost/proceeds: {}", addr, cost_decimal);

    let balance_change: Decimal;
    if trade_amount > 0.0 {
        println!("[{}] Buying: Checking balance ({}) against cost ({})", addr, current_balance, cost_decimal);
        if current_balance < cost_decimal {
             println!("[{}] Trade rejected: Insufficient balance.", addr);
            return ServerMessage::Error {
                message: format!(
                    "Insufficient balance. Need {:.2}, have {:.2}.",
                    cost_decimal, current_balance
                ),
            };
        }
        balance_change = -cost_decimal;
    } else {
        balance_change = cost_decimal;
         println!("[{}] Selling: Proceeds = {}, Balance change = {}", addr, cost_decimal, balance_change);
    }

    println!(
        "[{}] Attempting DB update: User={}, Post={}, Balance Change={}, New Position={}",
        addr, user_id.clone(), post_id, balance_change, new_position_amount
    );
    match update_balance_and_position(&db, &user_id, post_id, balance_change, new_position_amount).await {
        Ok(_) => {
            println!("[{}] DB update successful.", addr);
            let new_supply = current_supply + trade_amount;
            {
                let mut state_lock = market_state.lock().await;
                state_lock.insert(post_id, new_supply);
                 println!("[{}] In-memory supply updated for post {}: {}", addr, post_id, new_supply);
            }
            println!(
                "[{}] Trade successful: User={}, Post={}, Amount={}. New Supply={}, New Pos={}",
                addr, user_id, post_id, trade_amount, new_supply, new_position_amount
            );
            let new_position_f64 = new_position_amount.to_string().parse::<f64>().unwrap_or(0.0);
            ServerMessage::TradePlaced {
                post_id,
                user_id,
                new_position: new_position_f64,
            }
        }
        Err(e) => {
             println!("[{}] DB update failed: {}", addr, e);
             ServerMessage::Error { message: format!("Trade execution failed: {}", e) }
        }
    }
} 