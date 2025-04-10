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
    supply: i64, // Can be negative
}

// Holds the details of a user's position in a specific post
#[derive(Debug, Clone, Default)]
struct UserPositionDetail {
    size: i64,
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

// Represents incoming messages from the client
#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    CreatePost { content: String },
    Buy { post_id: Uuid },  // Buy 1 unit
    Sell { post_id: Uuid }, // Sell 1 unit
}

// Used within UserSync to send position details
#[derive(Serialize, Debug, Clone)]
struct PositionDetail {
    post_id: Uuid,
    size: i64,
    average_price: f64,
    unrealized_pnl: f64,
}

// Represents messages sent from the server to the client
#[derive(Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    InitialState { posts: Vec<Post> },
    UserSync { 
        balance: f64,
        positions: Vec<PositionDetail>,
    },
    NewPost { post: Post },
    MarketUpdate { post_id: Uuid, price: f64, supply: i64 }, 
    BalanceUpdate { balance: f64 },
    PositionUpdate { // Update for a specific position after a trade
        post_id: Uuid,
        size: i64,
        average_price: f64,
        unrealized_pnl: f64,
    },
    Error { message: String },
}


// --- State ---

#[derive(Clone)]
struct AppState {
    clients: Clients,
    posts: Posts,
    user_balances: UserBalances,
    user_positions: UserPositions, // Updated type
    jwt_secret: Arc<String>,
}

// --- Calculation Helpers ---

fn calculate_average_price(position: &UserPositionDetail) -> f64 {
    if position.size == 0 {
        0.0
    } else {
        position.total_cost_basis / (position.size as f64)
    }
}

// Re-added: calculate_unrealized_pnl function
fn calculate_unrealized_pnl(
    position: &UserPositionDetail,
    current_market_price: f64,
) -> f64 {
    if position.size == 0 {
        0.0
    } else {
        let avg_price = calculate_average_price(position);
        // PNL = (Current Price - Average Price) * Size
        (current_market_price - avg_price) * (position.size as f64)
    }
}

// --- Bonding Curve Logic ---

fn get_price(supply: i64) -> f64 {
    if supply > 0 {
        1.0 + (supply as f64).powf(0.5)
    } else {
        1.0 / (1.0 + (supply.abs() as f64).powf(0.5))
    }
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

    // --- Send UserSync (Balance & Positions) ---
    let user_balance = *state.user_balances.entry(user_id.clone()).or_insert(INITIAL_BALANCE);
    let user_positions_detail: Vec<PositionDetail> = state
        .user_positions
        .get(&user_id)
        .map(|positions_map| {
            positions_map
                .iter()
                .filter_map(|pos_entry| {
                    let post_id = *pos_entry.key();
                    let position = pos_entry.value();

                    state.posts.get(&post_id).map(|market_post| {
                        let current_market_price = market_post.price.unwrap_or_else(|| get_price(market_post.supply));
                        PositionDetail {
                            post_id,
                            size: position.size,
                            average_price: calculate_average_price(position),
                            // Calculate PNL for UserSync
                            unrealized_pnl: calculate_unrealized_pnl(position, current_market_price),
                        }
                    // Filter out positions in posts that no longer exist (edge case)
                    }).filter(|detail| detail.size != 0) // Also ensure size is not 0
                })
                .collect()
        })
        .unwrap_or_else(Vec::new);

    let user_sync_msg = ServerMessage::UserSync {
        balance: user_balance,
        positions: user_positions_detail,
    };
     if client_sender.send(Ok(Message::text(serde_json::to_string(&user_sync_msg).unwrap()))).is_err() {
         eprintln!("Failed initial send (UserSync) to client_id={}", client_id);
         state.clients.remove(&client_id);
         return;
    }
     println!("Sent UserSync to client_id={}", client_id);
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
    new_supply: i64,
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
                 if position.size != 0 { // Only update if position exists
                     // Recalculate PNL with the new market price
                     let avg_price = calculate_average_price(&position);
                     let updated_pnl = calculate_unrealized_pnl(&position, new_price);

                     let position_update_msg = ServerMessage::PositionUpdate {
                         post_id,
                         size: position.size,
                         average_price: avg_price,
                         unrealized_pnl: updated_pnl,
                     };
                     println!(
                         "   -> Sending PNL update for Post {} to OTHER User {} ({}): PNL {:.4}",
                         post_id, user_id, current_client_id, updated_pnl
                     );
                     // Use the existing send_to_client helper
                      send_to_client(current_client_id, position_update_msg, state).await;
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
                        // --- Create Post Logic (Mostly unchanged) ---
                        let new_post_id = Uuid::new_v4();
                        let initial_price = get_price(0);
                        let new_post = Post {
                            id: new_post_id,
                            user_id: user_id.to_string(),
                            content,
                            timestamp: Utc::now(),
                            supply: 0,
                            price: Some(initial_price), // Store initial price
                        };
                        state.posts.insert(new_post_id, new_post.clone());
                        println!(
                            "-> Post {} created (Price: {:.4}, Supply: 0)",
                            new_post_id, initial_price
                        );
                        // No need to recalculate price before broadcast here
                        let broadcast_msg = ServerMessage::NewPost { post: new_post };
                        broadcast_message(broadcast_msg, state).await;
                    }
                    ClientMessage::Buy { post_id } => {
                        if let Some(mut post_entry) = state.posts.get_mut(&post_id) {
                            let post = &mut *post_entry;
                            let amount_to_buy: i64 = 1;
                            let new_supply = post.supply + amount_to_buy;
                            let new_price = get_price(new_supply);
                            let cost = new_price * (amount_to_buy as f64);

                            let mut balance_entry = state.user_balances.entry(user_id.to_string()).or_insert(INITIAL_BALANCE);
                            let user_balance = balance_entry.value_mut();

                            if *user_balance >= cost {
                                *user_balance -= cost;
                                post.supply = new_supply;
                                post.price = Some(new_price);

                                let user_positions_for_user = state.user_positions.entry(user_id.to_string()).or_default();
                                let mut user_position = user_positions_for_user.entry(post_id).or_insert_with(UserPositionDetail::default);
                                user_position.size += amount_to_buy;
                                user_position.total_cost_basis += cost;

                                // Calculate post-trade metrics (Avg Price only)
                                let avg_price = calculate_average_price(&user_position);
                                let pnl = calculate_unrealized_pnl(&user_position, new_price); // Calculate PNL after trade

                                println!(
                                    "-> Buy OK (Cost: {:.4}): Post {} (Supply: {}, Price: {:.4}), User {} (Pos: {}, Avg Prc: {:.4}, Bal: {:.4})",
                                    cost, post_id, post.supply, new_price, user_id, user_position.size, avg_price, *user_balance
                                );

                                // Send updates to the user
                                send_to_client(client_id, ServerMessage::BalanceUpdate { balance: *user_balance }, state).await;
                                send_to_client(client_id, ServerMessage::PositionUpdate {
                                    post_id,
                                    size: user_position.size,
                                    average_price: avg_price,
                                    unrealized_pnl: pnl, // Send updated PNL
                                 }, state).await;

                                // Broadcast market update AND trigger PNL updates for OTHERS
                                broadcast_market_and_position_updates(post_id, new_price, post.supply, client_id, state).await;

                            } else {
                                println!(
                                    "-> Buy FAIL: User {} Insufficient balance ({:.4}) for cost {:.4} (at new price {:.4})",
                                     user_id, *user_balance, cost, new_price
                                );
                                send_to_client(client_id, ServerMessage::Error { message: format!("Insufficient balance ({:.4}) to buy at cost {:.4} (price would become {:.4})", *user_balance, cost, new_price) }, state).await;
                            }
                        } else {
                            println!("-> Buy FAIL: Post {} not found", post_id);
                            send_to_client(client_id, ServerMessage::Error { message: format!("Post {} not found", post_id) }, state).await;
                        }
                    }
                    ClientMessage::Sell { post_id } => {
                        if let Some(mut post_entry) = state.posts.get_mut(&post_id) {
                            let post = &mut *post_entry;
                            let amount_to_sell: i64 = 1;
                            let new_supply = post.supply - amount_to_sell;
                            let new_price = get_price(new_supply);
                            let proceeds_or_cost = new_price * (amount_to_sell as f64);

                            let mut balance_entry = state.user_balances.entry(user_id.to_string()).or_insert(INITIAL_BALANCE);
                            let user_balance = balance_entry.value_mut();
                            let user_positions_for_user = state.user_positions.entry(user_id.to_string()).or_default();
                            let mut user_position = user_positions_for_user.entry(post_id).or_insert_with(UserPositionDetail::default);

                            // Determine cost/proceeds based on direction
                            let trade_cost: f64; // Cost to user (negative if receiving proceeds)
                            if user_position.size >= amount_to_sell {
                                // Selling existing long
                                trade_cost = -proceeds_or_cost; // Negative cost = proceeds
                            } else {
                                // Opening/increasing short
                                trade_cost = proceeds_or_cost; // Positive cost for short collateral
                                if *user_balance < trade_cost {
                                    println!(
                                        "-> Sell FAIL (Short): User {} Insufficient balance ({:.4}) for collateral {:.4} (at new price {:.4})",
                                        user_id, *user_balance, trade_cost, new_price
                                    );
                                    send_to_client(client_id, ServerMessage::Error { message: format!("Insufficient balance ({:.4}) to cover short collateral {:.4} (price would become {:.4})", *user_balance, trade_cost, new_price) }, state).await;
                                    return;
                                }
                            }
                            // If we reach here, the trade is allowed

                            *user_balance -= trade_cost; // Apply cost/proceeds
                            post.supply = new_supply;
                            post.price = Some(new_price);
                            user_position.size -= amount_to_sell;
                            // Update cost basis: Subtract cost if long, add cost if short (double negative)
                            user_position.total_cost_basis -= trade_cost; 

                            // Calculate post-trade metrics (Avg Price only)
                            let avg_price = calculate_average_price(&user_position);
                            let pnl = calculate_unrealized_pnl(&user_position, new_price); // Calculate PNL after trade

                             println!(
                                "-> Sell OK (Value: {:.4}): Post {} (Supply: {}, Price: {:.4}), User {} (Pos: {}, Avg Prc: {:.4}, Bal: {:.4})",
                                -trade_cost, 
                                post_id, post.supply, new_price, user_id, user_position.size, avg_price, *user_balance
                            );

                            // Send updates to the user
                            send_to_client(client_id, ServerMessage::BalanceUpdate { balance: *user_balance }, state).await;
                             send_to_client(client_id, ServerMessage::PositionUpdate {
                                    post_id,
                                    size: user_position.size,
                                    average_price: avg_price,
                                    unrealized_pnl: pnl, // Send updated PNL
                                 }, state).await;

                            // Broadcast market update AND trigger PNL updates for OTHERS
                            broadcast_market_and_position_updates(post_id, new_price, post.supply, client_id, state).await;

                        } else {
                            println!("-> Sell FAIL: Post {} not found", post_id);
                            send_to_client(client_id, ServerMessage::Error { message: format!("Post {} not found", post_id) }, state).await;
                        }
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
