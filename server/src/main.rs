use chrono::{DateTime, Utc};
use dashmap::DashMap;
use dotenvy::dotenv;
use futures_util::StreamExt;
use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, env, sync::Arc};
use tokio::sync::mpsc::{self, UnboundedSender};
use uuid::Uuid;
use warp::{
    filters::ws::{Message, WebSocket},
    http::StatusCode,
    Filter, Rejection, Reply,
};

// --- Constants ---
const INITIAL_BALANCE: f64 = 1000.0;
const BONDING_CURVE_EPSILON: f64 = 1e-9;
const EPSILON: f64 = 1e-9; // General purpose epsilon for float comparisons

// --- Types ---

// Represents the claims expected in the Supabase JWT
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String, // Subject (user ID)
    aud: String, // Audience
    exp: usize,  // Expiration time
}

// Represents a post in the timeline
#[derive(Debug, Serialize, Clone)]
struct Post {
    id: Uuid,
    user_id: String,
    content: String,
    timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")] // Only include price if calculated
    price: Option<f64>,
    supply: f64, // <-- Changed to f64
}

// Holds the details of a user's position in a specific post
#[derive(Debug, Clone, Default)]
struct UserPositionDetail {
    size: f64,
    total_cost_basis: f64,
}

// Structure to hold client-specific information
#[derive(Debug)]
struct Client {
    user_id: String,
    sender: UnboundedSender<Result<Message, warp::Error>>,
}

// Type aliases for shared state
type Clients = Arc<DashMap<Uuid, Client>>;         // ClientID -> Client
type Posts = Arc<DashMap<Uuid, Post>>;             // PostID -> Post
type UserBalances = Arc<DashMap<String, f64>>;   // UserID -> Balance
// UserID -> PostID -> UserPositionDetail
type UserPositions = Arc<DashMap<String, DashMap<Uuid, UserPositionDetail>>>;
type UserRealizedPnl = Arc<DashMap<String, f64>>; // Added: UserID -> Total Realized PNL

// Represents incoming messages from the client
#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    CreatePost { content: String },
    Buy { post_id: Uuid, quantity: f64 }, // <-- Added quantity
    Sell { post_id: Uuid, quantity: f64 }, // <-- Added quantity
}

// Used within UserSync to send position details
#[derive(Serialize, Debug, Clone)]
struct PositionDetail {
    post_id: Uuid,
    size: f64, // <-- Changed to f64
    average_price: f64,
    unrealized_pnl: f64,
}

// Represents messages sent from the server to the client
#[derive(Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    InitialState { posts: Vec<Post> },
    UserSync { 
        margin: f64,
        balance: f64,
        positions: Vec<PositionDetail>,
        total_realized_pnl: f64, // Added
    },
    NewPost { post: Post },
    MarketUpdate { post_id: Uuid, price: f64, supply: f64 }, // <-- Changed supply to f64
    BalanceUpdate { balance: f64 },
    PositionUpdate { // Update for a specific position after a trade
        post_id: Uuid,
        size: f64, // <-- Changed size to f64
        average_price: f64,
        unrealized_pnl: f64,
    },
    RealizedPnlUpdate { total_realized_pnl: f64 }, // Added
    MarginUpdate { margin: f64 }, // Added
    Error { message: String },
}


// --- State ---

#[derive(Clone)]
struct AppState {
    clients: Clients,
    posts: Posts,
    user_balances: UserBalances,
    user_positions: UserPositions, // Updated type
    user_realized_pnl: UserRealizedPnl, // Added
    jwt_secret: Arc<String>,
}

// --- Calculation Helpers ---

fn calculate_average_price(position: &UserPositionDetail) -> f64 {
    // Use a small epsilon to handle potential floating point inaccuracies near zero
    if position.size.abs() < 1e-9 {
        0.0
    } else {
        position.total_cost_basis / position.size
    }
}

// Re-added: calculate_unrealized_pnl function
fn calculate_unrealized_pnl(
    position: &UserPositionDetail,
    current_market_price: f64,
) -> f64 {
     if position.size.abs() < 1e-9 {
        0.0
    } else {
        let avg_price = calculate_average_price(position);
        // PNL = (Current Price - Average Price) * Size
        (current_market_price - avg_price) * position.size
    }
}

// --- Margin Calculation Helper ---

fn calculate_user_margin(user_id: &str, state: &AppState) -> f64 {
    let balance = state.user_balances.get(user_id).map_or(INITIAL_BALANCE, |b| *b.value());
    let mut total_unrealized_pnl = 0.0;

    if let Some(user_positions_map) = state.user_positions.get(user_id) {
        for position_entry in user_positions_map.iter() {
            let post_id = *position_entry.key();
            let position = position_entry.value();

            if position.size.abs() > EPSILON {
                // Get current market price for the post
                if let Some(market_post) = state.posts.get(&post_id) {
                    let current_market_price = market_post.price.unwrap_or_else(|| get_price(market_post.supply));
                    total_unrealized_pnl += calculate_unrealized_pnl(position, current_market_price);
                } else {
                    // Should not happen if data is consistent, but handle defensively
                    eprintln!("Warning: Post {} not found while calculating margin for user {}", post_id, user_id);
                }
            }
        }
    }
    // Margin = Balance + Total Unrealized PNL
    balance + total_unrealized_pnl
}

// --- Bonding Curve Logic ---

// Price function P(s)
fn get_price(supply: f64) -> f64 {
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
// Let s = -t (t>0). P(x) = 1 / (1+sqrt(|x|)). Int P(x) dx from -t to 0.
// Substitute u=sqrt(|x|)=sqrt(-x), u^2=-x, 2udu = -dx
// Int (1/(1+u)) * (-2u du) from sqrt(t) to 0
// = Int (2u / (1+u)) du from 0 to sqrt(t)
// = Int (2 - 2/(1+u)) du from 0 to sqrt(t)
// = [2u - 2ln(1+u)] from 0 to sqrt(t)
// = 2sqrt(t) - 2ln(1+sqrt(t)) = 2sqrt(|s|) - 2ln(1+sqrt(|s|))
fn integral_neg_to_zero(s: f64) -> f64 {
    if s >= -BONDING_CURVE_EPSILON { // Treat s>=0 as 0
        0.0
    } else {
        let t = s.abs(); // t = |s|
        2.0 * t.sqrt() - 2.0 * (1.0 + t.sqrt()).ln()
    }
}

// Calculate the cost (definite integral) of changing supply from s1 to s2
// Cost = Integral[s1, s2] P(x) dx
//      = Integral[0, s2] P(x) dx - Integral[0, s1] P(x) dx
// Where Integral[0, s] = integral_pos(s) if s > 0
// And   Integral[0, s] = -Integral[s, 0] P(x) dx = -integral_neg_to_zero(s) if s < 0
fn calculate_cost(s1: f64, s2: f64) -> f64 {
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

// --- Authentication ---

// Structure to deserialize the query parameter containing the token
#[derive(Deserialize, Debug)]
struct AuthQuery {
    token: String,
}

// Function to validate the JWT
fn validate_token(token: &str, secret: &str) -> Result<Claims, String> {
    let key = DecodingKey::from_secret(secret.as_ref());
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true; // Check expiration
    validation.set_audience(&["authenticated"]); // Verify audience

    decode::<Claims>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|err| format!("JWT validation failed: {}", err))
}

// Warp filter to extract token, validate it, and pass user_id
fn with_auth(
    state: AppState,
) -> impl Filter<Extract = (String,), Error = Rejection> + Clone {
    warp::query::<AuthQuery>()
        .and(warp::any().map(move || state.clone()))
        .and_then(|query: AuthQuery, current_state: AppState| async move {
            match validate_token(&query.token, &current_state.jwt_secret) {
                Ok(claims) => {
                    // Check if 'sub' claim exists and is a non-empty string
                     if claims.sub.is_empty() {
                         eprintln!("JWT validation error: Missing or empty 'sub' claim.");
                         Err(warp::reject::custom(AuthError::InvalidToken))
                     } else {
                        println!("JWT validated for user: {}", claims.sub);
                        Ok(claims.sub) // Pass the user_id (sub)
                     }
                }
                Err(e) => {
                    eprintln!("JWT validation error: {}", e);
                    Err(warp::reject::custom(AuthError::InvalidToken))
                }
            }
        })
}

// --- WebSocket Handling ---

// Helper to send a message to a specific client
async fn send_to_client(client_id: Uuid, message: ServerMessage, state: &AppState) {
    if let Some(client) = state.clients.get(&client_id) {
        match serde_json::to_string(&message) {
            Ok(json_msg) => {
                if client.sender.send(Ok(Message::text(json_msg))).is_err() {
                    // Error sending means channel is likely closed, client will be removed soon
                    eprintln!(
                        "Error queueing message type '{:?}' for client_id={}",
                        message_type_for_debug(&message),
                        client_id
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "Failed to serialize direct message '{:?}' for client_id={}: {}",
                     message_type_for_debug(&message),
                     client_id, e
                );
            }
        }
    } else {
        // This might happen if client disconnected between message generation and sending
         eprintln!(
            "Attempted to send direct message '{:?}' to non-existent client_id={}",
            message_type_for_debug(&message),
            client_id
        );
    }
}

async fn handle_connection(ws: WebSocket, user_id: String, state: AppState) {
    let client_id = Uuid::new_v4();
    println!(
        "New WebSocket connection: client_id={}, user_id={}",
        client_id, &user_id // Borrow user_id here
    );

    let (client_sender, client_rcv) = mpsc::unbounded_channel();
    let client_rcv_stream = tokio_stream::wrappers::UnboundedReceiverStream::new(client_rcv);

    state.clients.insert(
        client_id,
        Client {
            user_id: user_id.clone(), // Clone for the Client struct
            sender: client_sender.clone(),
        },
    );

    // --- Send InitialState (Global Posts) --- 
    let current_posts: Vec<Post> = state
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

    // --- Send UserSync (Balance, Positions, Realized PNL) ---
    let user_balance = *state.user_balances.entry(user_id.clone()).or_insert(INITIAL_BALANCE);
    // Fetch realized PNL, defaulting to 0.0 if user not found
    let total_realized_pnl = *state.user_realized_pnl.entry(user_id.clone()).or_insert(0.0);
    // Calculate initial margin
    let initial_margin = calculate_user_margin(&user_id, &state); // Use helper
    let user_positions_detail: Vec<PositionDetail> = state
        .user_positions
        .get(&user_id)
        .map(|positions_map| {
            positions_map
                .iter()
                .filter_map(|pos_entry| {
                    let post_id = *pos_entry.key();
                    let position = pos_entry.value();

                    // Only include positions with non-zero size (using epsilon)
                    if position.size.abs() <= EPSILON {
                         return None; 
                    }

                    state.posts.get(&post_id).map(|market_post| {
                        let current_market_price = market_post.price.unwrap_or_else(|| get_price(market_post.supply));
                        
                        // Determine the average price to display
                        let avg_price = calculate_average_price(position);

                        PositionDetail {
                            post_id,
                            size: position.size,
                            average_price: avg_price.abs(), // Send calculated avg price (abs)
                            // Calculate PNL for UserSync
                            unrealized_pnl: calculate_unrealized_pnl(position, current_market_price),
                        }
                    })
                })
                .collect()
        })
        .unwrap_or_else(Vec::new);

    let user_sync_msg = ServerMessage::UserSync {
        margin: initial_margin, // Add margin
        balance: user_balance,
        positions: user_positions_detail,
        total_realized_pnl, // Send fetched/defaulted value
    };
     if client_sender.send(Ok(Message::text(serde_json::to_string(&user_sync_msg).unwrap()))).is_err() {
         eprintln!("Failed initial send (UserSync) to client_id={}", client_id);
         state.clients.remove(&client_id);
         return;
    }
     println!("Sent UserSync to client_id={} (Bal: {:.4}, RPnl: {:.4})", client_id, user_balance, total_realized_pnl);
    // --------------------------

    let (ws_sender, mut ws_receiver) = ws.split();

    // Task to forward messages from MPSC channel to WebSocket sink
    tokio::spawn(async move {
       // Use explicit borrowing for client_id in the closure
        let task_client_id = client_id;
        if let Err(e) = client_rcv_stream.forward(ws_sender).await {
            eprintln!(
                "Error sending message via MPSC->WS forwarder for client {}: {}",
                task_client_id, e
            );
        }
        println!("MPSC->WS forwarder task finished for client {}", task_client_id);
    });

    // Main loop to process incoming messages
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

        handle_client_message(client_id, &user_id, msg, &state).await; // Pass state by ref
    }

    println!(
        "WebSocket connection closed for client_id={}, user_id={}",
        client_id, &user_id
    );
    state.clients.remove(&client_id);
}

// Function to broadcast market update and then send individual position updates to OTHER clients
async fn broadcast_market_and_position_updates(
    post_id: Uuid,
    new_price: f64,
    new_supply: f64,
    trading_client_id: Uuid, // ID of the client who made the trade
    state: &AppState,
) {
    // 1. Broadcast the general market update to everyone
    let market_update_msg = ServerMessage::MarketUpdate {
        post_id,
        price: new_price,
        supply: new_supply,
    };
    // Use original broadcast_message which sends to all clients
    broadcast_message(market_update_msg, state).await;

    // 2. Iterate through all ACTIVE clients to potentially send PNL updates
    for client_entry in state.clients.iter() {
        let current_client_id = *client_entry.key();
        let client_info = client_entry.value();
        let user_id = &client_info.user_id;

        // Skip the client who initiated this trade
        if current_client_id == trading_client_id {
            continue;
        }

        // Check if this *other* user has a position in the updated post
         if let Some(user_positions_map) = state.user_positions.get(user_id) {
             if let Some(position) = user_positions_map.get(&post_id) {
                 // Only update if position exists and is non-zero
                 if position.size.abs() > EPSILON { 
                     // Determine the average price to display for this other user
                     let avg_price = calculate_average_price(&position);
                     let updated_pnl = calculate_unrealized_pnl(&position, new_price);

                     let position_update_msg = ServerMessage::PositionUpdate {
                         post_id,
                         size: position.size,
                         average_price: avg_price.abs(), // Send calculated avg price (abs)
                         unrealized_pnl: updated_pnl,
                     };
                     println!(
                         "   -> Sending PNL update for Post {} to OTHER User {} ({}): AvgPrc: {:.4}, PNL {:.4}",
                         post_id, user_id, current_client_id, avg_price, updated_pnl
                     );
                     // Use the existing send_to_client helper
                      send_to_client(current_client_id, position_update_msg, state).await;

                     // Send Margin Update for the *other* user
                     let other_user_margin = calculate_user_margin(user_id, state);
                     send_to_client(current_client_id, ServerMessage::MarginUpdate { margin: other_user_margin }, state).await;
                 }
             }
         }
    }
}

// Original broadcast function (used for NewPost and MarketUpdate inside the func above)
async fn broadcast_message(message: ServerMessage, state: &AppState) {
     if state.clients.is_empty() {
        println!("No clients connected, skipping broadcast.");
        return;
    }
    let serialized_message = match serde_json::to_string(&message) {
         // ... (serialization and error check) ...
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
         // ... (send logic) ...
        let client_id = client_entry.key();
        let client = client_entry.value();
        if client.sender.send(Ok(Message::text(serialized_message.clone()))).is_err() {
            eprintln!("Failed to send broadcast message to client_id={}, user_id={}. Channel likely closed.", client_id, client.user_id);
        }
    }
}

// Helper to get simple message type string for logging
fn message_type_for_debug(msg: &ServerMessage) -> &'static str {
     match msg {
        ServerMessage::InitialState { .. } => "InitialState",
        ServerMessage::UserSync { .. } => "UserSync",
        ServerMessage::NewPost { .. } => "NewPost",
        ServerMessage::MarketUpdate { .. } => "MarketUpdate",
        ServerMessage::BalanceUpdate { .. } => "BalanceUpdate",
        ServerMessage::PositionUpdate { .. } => "PositionUpdate",
        ServerMessage::RealizedPnlUpdate { .. } => "RealizedPnlUpdate", // Added
        ServerMessage::MarginUpdate { .. } => "MarginUpdate", // Added
        ServerMessage::Error { .. } => "Error",
    }
}

async fn handle_client_message(
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
                        // --- Create Post Logic ---
                        let new_post_id = Uuid::new_v4();
                        let initial_price = get_price(0.0); // Use f64
                        let new_post = Post {
                            id: new_post_id,
                            user_id: user_id.to_string(),
                            content,
                            timestamp: Utc::now(),
                            supply: 0.0, // Use f64
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
                    ClientMessage::Buy { post_id, quantity } => {
                         if quantity <= EPSILON {
                            println!("-> Buy FAIL: Quantity {} must be positive", quantity);
                            send_to_client(client_id, ServerMessage::Error { message: format!("Buy quantity ({:.6}) must be positive", quantity) }, state).await;
                            return;
                        }

                        // --- Refactor Step 1: Read needed state --- 
                        let maybe_post_data = state.posts.get(&post_id).map(|p| (p.supply, p.price));
                        let user_balance_val = state.user_balances.get(user_id).map_or(INITIAL_BALANCE, |b| *b.value());
                        let user_position_val = state.user_positions.get(user_id)
                            .and_then(|positions| positions.get(&post_id).map(|pos_detail| pos_detail.clone())); // Clone to release lock
                        
                        // --- Refactor Step 2: Perform checks and calculations --- 
                        
                        // Check if post exists
                        let (current_supply, _) = match maybe_post_data {
                            Some((supply, _)) => (supply, ()), // Ignore price for now, get it after potential update
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

                        // Check balance
                        if user_balance_val < trade_cost {
                             println!(
                                "-> Buy FAIL: User {} Insufficient balance ({:.6}) for cost {:.6} (Quantity: {:.6})",
                                user_id, user_balance_val, trade_cost, quantity
                            );
                            send_to_client(client_id, ServerMessage::Error { message: format!("Insufficient balance ({:.6}) for buy cost {:.6}", user_balance_val, trade_cost) }, state).await;
                            return;
                        }
                        
                        // Calculate PNL if covering short
                        let old_position = user_position_val.unwrap_or_default(); // Use default if no previous position
                        let old_size = old_position.size;
                        let mut realized_pnl_for_trade = 0.0;
                        let mut basis_change_for_short_cover = 0.0;
                        let mut avg_short_entry_price = 0.0; // For logging
                        let mut avg_cost_of_buy_reduction = 0.0; // For logging

                        if old_size < -EPSILON {
                            let reduction_amount = quantity.min(old_size.abs());
                            if reduction_amount > EPSILON {
                                avg_short_entry_price = if old_size.abs() > EPSILON { old_position.total_cost_basis / old_size } else { 0.0 };
                                let exact_cost_for_reduction = calculate_cost(current_supply, current_supply + reduction_amount);
                                avg_cost_of_buy_reduction = if reduction_amount.abs() > EPSILON { exact_cost_for_reduction / reduction_amount } else { 0.0 };
                                
                                realized_pnl_for_trade = (avg_short_entry_price - avg_cost_of_buy_reduction) * reduction_amount;
                                basis_change_for_short_cover = avg_short_entry_price * reduction_amount; // Amount to add back to basis
                            }
                        }
                        
                        // --- Refactor Step 3: Acquire locks and update state --- 
                        let mut total_pnl_updated = false; // Declare outside the scope
                        let final_price;
                        let final_position;
                        let final_balance;
                        let final_total_realized_pnl;
                        
                        { // Scope for mutable references
                            // Lock Post
                             let mut post_entry = match state.posts.get_mut(&post_id) {
                                 Some(entry) => entry,
                                 None => return, // Should have been caught earlier, but defensive
                             };
                             let post = &mut *post_entry;
                             post.supply = new_supply;
                             final_price = get_price(new_supply); // Calculate final price *after* supply update
                             post.price = Some(final_price);

                            // Lock Balance
                            let mut balance_entry = state.user_balances.entry(user_id.to_string()).or_insert(INITIAL_BALANCE);
                            *balance_entry -= trade_cost;
                            final_balance = *balance_entry;
                            
                            // Lock Position
                            let user_positions_for_user = state.user_positions.entry(user_id.to_string()).or_default();
                            let mut user_position = user_positions_for_user.entry(post_id).or_insert_with(UserPositionDetail::default);
                            user_position.size += quantity;

                            // Update basis based on calculations done earlier
                             if old_size < -EPSILON {
                                 user_position.total_cost_basis += basis_change_for_short_cover;
                                  println!(
                                    "   -> Covering short: Reduction: {:.6}, Avg Short Entry Prc: {:.6}, Avg Buy Cost (Reduction Only): {:.6}, Basis Change: +{:.6}, Pnl: {:.6}",
                                    quantity.min(old_size.abs()), avg_short_entry_price, avg_cost_of_buy_reduction, basis_change_for_short_cover, realized_pnl_for_trade
                                  );
                             }

                            // Add cost basis for long part
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
                            
                            // Update Realized PNL if necessary
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

                            // Reset basis if flat
                             if user_position.size.abs() < EPSILON {
                                 println!("   -> Position size is near zero ({:.8}) after trade, resetting basis.", user_position.size);
                                 user_position.size = 0.0;
                                 user_position.total_cost_basis = 0.0;
                             }

                            final_position = user_position.clone(); // Clone final state for logging/sending
                             
                        } // Mutable references are dropped here
                        
                        // --- Refactor Step 4: Log results and send updates --- 
                        let display_avg_price = calculate_average_price(&final_position);
                        let unrealized_pnl = calculate_unrealized_pnl(&final_position, final_price);

                         println!(
                            "-> Buy OK (Qty: {:.6}, Cost: {:.6}): Post {} (Supply: {:.6}, Price: {:.6}), User {} (Pos: {:.6}, Avg Prc: {:.6}, URPnl: {:.6}, Bal: {:.6}) RPnlTrade: {:.6}",
                            quantity, trade_cost, post_id, new_supply, final_price, user_id, final_position.size, 
                            display_avg_price, unrealized_pnl, final_balance, realized_pnl_for_trade
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
                         
                        // Note: Previous error handling for balance/post not found happens earlier now
                    
                    }
                     ClientMessage::Sell { post_id, quantity } => {
                        if quantity <= EPSILON {
                             println!("-> Sell FAIL: Quantity {} must be positive", quantity);
                            send_to_client(client_id, ServerMessage::Error { message: format!("Sell quantity ({:.6}) must be positive", quantity) }, state).await;
                            return;
                        }

                        // --- Refactor Step 1: Read needed state --- 
                        let maybe_post_data = state.posts.get(&post_id).map(|p| (p.supply, p.price));
                        let user_balance_val = state.user_balances.get(user_id).map_or(INITIAL_BALANCE, |b| *b.value());
                        let user_position_val = state.user_positions.get(user_id)
                            .and_then(|positions| positions.get(&post_id).map(|pos_detail| pos_detail.clone())); // Clone to release lock

                        // --- Refactor Step 2: Perform checks and calculations --- 
                        
                        // Check if post exists
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

                        // Check balance ONLY if opening/increasing short
                        if old_size <= EPSILON && quantity > EPSILON { // User is flat or short, and selling
                            let cost_to_open_short = trade_proceeds; // Proceeds act as collateral
                            if user_balance_val < cost_to_open_short {
                                println!(
                                    "-> Sell FAIL (Short): User {} Insufficient balance ({:.6}) for collateral {:.6} (Quantity: {:.6})",
                                    user_id, user_balance_val, cost_to_open_short, quantity
                                );
                                send_to_client(client_id, ServerMessage::Error { message: format!("Insufficient balance ({:.6}) to cover short collateral {:.6}", user_balance_val, cost_to_open_short) }, state).await;
                                return;
                            }
                        }
                        
                        // Calculate PNL if closing long
                        let mut realized_pnl_for_trade = 0.0;
                        let mut basis_removed = 0.0;
                        let mut avg_price_before_long_close = 0.0; // For logging
                        let mut avg_proceeds_of_sell_reduction = 0.0; // For logging

                        if old_size > EPSILON {
                            let reduction_amount = quantity.min(old_size);
                            if reduction_amount > EPSILON {
                                avg_price_before_long_close = calculate_average_price(&old_position);
                                let exact_proceeds_for_reduction = calculate_cost(current_supply - reduction_amount, current_supply);
                                avg_proceeds_of_sell_reduction = if reduction_amount.abs() > EPSILON { exact_proceeds_for_reduction / reduction_amount } else { 0.0 };
                                realized_pnl_for_trade = (avg_proceeds_of_sell_reduction - avg_price_before_long_close) * reduction_amount;
                                basis_removed = avg_price_before_long_close * reduction_amount; // Amount to remove from basis
                            }
                        }

                        // --- Refactor Step 3: Acquire locks and update state --- 
                        let mut total_pnl_updated = false; // Declare outside scope
                        let final_price;
                        let final_position;
                        let final_balance;
                        let final_total_realized_pnl;
                        
                        { // Scope for mutable references
                            // Lock Post
                             let mut post_entry = match state.posts.get_mut(&post_id) {
                                 Some(entry) => entry,
                                 None => return, // Should be caught earlier
                             };
                             let post = &mut *post_entry;
                             post.supply = new_supply;
                             final_price = get_price(new_supply);
                             post.price = Some(final_price);

                            // Lock Balance
                            let mut balance_entry = state.user_balances.entry(user_id.to_string()).or_insert(INITIAL_BALANCE);
                            *balance_entry += trade_proceeds; // Add proceeds from sell
                            final_balance = *balance_entry;
                            
                            // Lock Position
                            let user_positions_for_user = state.user_positions.entry(user_id.to_string()).or_default();
                            let mut user_position = user_positions_for_user.entry(post_id).or_insert_with(UserPositionDetail::default);
                            user_position.size -= quantity;

                            // Update basis based on calculations done earlier
                             if old_size > EPSILON {
                                 user_position.total_cost_basis -= basis_removed;
                                 println!("   -> Selling long: Reduction: {:.6}, Avg Buy Prc: {:.6}, Avg Sell Prc (Reduction Only): {:.6}, Basis Removed: {:.6}, Pnl: {:.6}",
                                    quantity.min(old_size), avg_price_before_long_close, avg_proceeds_of_sell_reduction, basis_removed, realized_pnl_for_trade);
                             }

                            // Adjust basis for short part
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

                            // Update Realized PNL if necessary
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

                            // Reset basis if flat
                             if user_position.size.abs() < EPSILON {
                                 println!("   -> Position size is near zero ({:.8}) after trade, resetting basis.", user_position.size);
                                 user_position.size = 0.0;
                                 user_position.total_cost_basis = 0.0;
                             }
                             
                            final_position = user_position.clone();
                        } // Mutable references are dropped here
                        
                        // --- Refactor Step 4: Log results and send updates --- 
                        let display_avg_price = calculate_average_price(&final_position);
                        let unrealized_pnl = calculate_unrealized_pnl(&final_position, final_price);

                         println!(
                            "-> Sell OK (Qty: {:.6}, Proceeds: {:.6}): Post {} (Supply: {:.6}, Price: {:.6}), User {} (Pos: {:.6}, Avg Prc: {:.6}, URPnl: {:.6}, Bal: {:.6}) RPnlTrade: {:.6}",
                            quantity, trade_proceeds,
                            post_id, new_supply, final_price, user_id, final_position.size, 
                            display_avg_price, unrealized_pnl, final_balance, realized_pnl_for_trade
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
                }
            }
            Err(e) => {
                eprintln!("Deserialize error for client_id={}: {}, err={}", client_id, text, e);
                // Use the helper function to send error to specific client
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


// --- Error Handling ---

#[derive(Debug)]
enum AuthError {
    InvalidToken,
}

impl warp::reject::Reject for AuthError {}

async fn handle_rejection(err: Rejection) -> Result<impl Reply, Infallible> {
    eprintln!("Handling rejection: {:?}", err); // Log the full rejection details

    if err.is_not_found() {
        Ok(warp::reply::with_status("NOT_FOUND", StatusCode::NOT_FOUND))
    } else if let Some(_) = err.find::<AuthError>() {
        Ok(warp::reply::with_status(
            "UNAUTHORIZED",
            StatusCode::UNAUTHORIZED,
        ))
    } else if let Some(_) = err.find::<warp::reject::MethodNotAllowed>() {
        Ok(warp::reply::with_status(
            "METHOD_NOT_ALLOWED",
            StatusCode::METHOD_NOT_ALLOWED,
        ))
     } else if err.find::<warp::reject::InvalidQuery>().is_some() {
         // Add specific handling for InvalidQuery if we want a different status code
         Ok(warp::reply::with_status(
             "BAD_REQUEST - Missing or invalid token query parameter",
             StatusCode::BAD_REQUEST,
         ))
    } else {
        // Keep logging for unhandled cases before returning 500
        eprintln!("Unhandled rejection type, returning 500: {:?}", err);
        Ok(warp::reply::with_status(
            "INTERNAL_SERVER_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}


// --- Main ---

#[tokio::main]
async fn main() {
     if dotenvy::from_filename("../.env").is_err() {
         if dotenv().is_err() {
             eprintln!("Warning: .env file not found.");
         }
     }

    let jwt_secret = env::var("JTW_SECRET").expect("JTW_SECRET must be set in .env file");

    // Initialize shared state
    let app_state = AppState {
        clients: Clients::default(),
        posts: Posts::default(),
        user_balances: UserBalances::default(),
        user_positions: UserPositions::default(), // Uses updated type alias
        user_realized_pnl: UserRealizedPnl::default(), // Added init
        jwt_secret: Arc::new(jwt_secret),
    };

    println!("JWT Secret loaded.");

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(with_auth(app_state.clone()))
        .and(warp::any().map(move || app_state.clone()))
        .map(|ws: warp::ws::Ws, user_id: String, state: AppState| {
            ws.on_upgrade(move |websocket| handle_connection(websocket, user_id, state))
        });

    let health_route = warp::path!("health").map(|| StatusCode::OK);

    let routes = health_route.or(ws_route).recover(handle_rejection);

    let addr = "127.0.0.1:8080";
    println!("Server starting on {}", addr);

    warp::serve(routes)
        .run(addr.parse::<std::net::SocketAddr>().unwrap())
        .await;
}

