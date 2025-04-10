use chrono::{DateTime, Utc};
use dashmap::DashMap;
use dotenvy::dotenv;
use futures_util::StreamExt;
use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, env, sync::Arc};
use tokio::sync::mpsc;
use uuid::Uuid;
use warp::{
    filters::ws::{Message, WebSocket},
    http::StatusCode,
    Filter, Rejection, Reply,
};

// --- Types ---

// Represents the claims expected in the Supabase JWT
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String, // Subject (user ID)
    aud: String, // Audience
    exp: usize,  // Expiration time
}

// Represents a post in the timeline
#[derive(Debug, Serialize, Clone)] // Clone needed for sending copies
struct Post {
    id: Uuid,
    user_id: String,
    content: String,
    timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")] // Only include price if calculated
    price: Option<f64>,
    supply: i64, // Can be negative
}

// Structure to hold client-specific information
#[derive(Debug)]
struct Client {
    user_id: String,
    sender: mpsc::UnboundedSender<Result<Message, warp::Error>>,
}

// Type aliases for shared state
type Clients = Arc<DashMap<Uuid, Client>>; // ClientID -> Client
type Posts = Arc<DashMap<Uuid, Post>>;   // PostID -> Post

// Represents incoming messages from the client
#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    CreatePost { content: String },
    Buy { post_id: Uuid },  // Buy 1 unit
    Sell { post_id: Uuid }, // Sell 1 unit
}

// Represents messages sent from the server to the client
#[derive(Serialize, Debug, Clone)] // Clone needed for broadcasting
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    // Initial state sent on connection
    InitialState { posts: Vec<Post> },
    // Broadcast when a new post is created
    NewPost { post: Post },
    // Broadcast when price/supply changes
    MarketUpdate { post_id: Uuid, price: f64, supply: i64 },
    // Error message sent to a specific client
    Error { message: String },
}


// --- State ---

#[derive(Clone)]
struct AppState {
    clients: Clients,
    posts: Posts,
    jwt_secret: Arc<String>,
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

async fn handle_connection(ws: WebSocket, user_id: String, state: AppState) {
    let client_id = Uuid::new_v4();
    println!(
        "New WebSocket connection: client_id={}, user_id={}",
        client_id, user_id
    );

    let (client_sender, client_rcv) = mpsc::unbounded_channel();
    let client_rcv_stream = tokio_stream::wrappers::UnboundedReceiverStream::new(client_rcv);

    state.clients.insert(
        client_id,
        Client {
            user_id: user_id.clone(),
            sender: client_sender.clone(), // Clone sender for state
        },
    );

    // --- Send initial state --- 
    // Collect all current posts, calculate their prices, and send
    let current_posts: Vec<Post> = state
        .posts
        .iter()
        .map(|entry| {
            let mut post = entry.value().clone();
            post.price = Some(get_price(post.supply));
            post
        })
        .collect();

    let initial_state_msg = ServerMessage::InitialState { posts: current_posts };
    if let Ok(json_msg) = serde_json::to_string(&initial_state_msg) {
         println!("Sending InitialState to client_id={}", client_id);
        // Send directly using the sender obtained for this specific client
        if client_sender.send(Ok(Message::text(json_msg))).is_err() {
            eprintln!(
                "Failed to send InitialState message to client_id={}",
                client_id
            );
             // If sending initial state fails, maybe disconnect early?
             state.clients.remove(&client_id);
             return; // Exit the handler
        }
    } else {
         eprintln!("Failed to serialize InitialState message");
          state.clients.remove(&client_id);
          return; // Exit the handler
    }
    // --------------------------

    let (ws_sender, mut ws_receiver) = ws.split();

    // Task to forward messages from MPSC channel to WebSocket sink
    tokio::spawn(async move {
        if let Err(e) = client_rcv_stream.forward(ws_sender).await {
            eprintln!(
                "Error sending message via MPSC->WS forwarder for client {}: {}",
                client_id, e
            );
        }
        println!("MPSC->WS forwarder task finished for client {}", client_id);
    });

    // Main loop to process incoming messages
    while let Some(result) = ws_receiver.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!(
                    "WebSocket error receiving message for client_id={}: {}, user_id={}",
                    client_id, e, user_id
                );
                break;
            }
        };

        handle_client_message(client_id, &user_id, msg, &state).await;
    }

    println!(
        "WebSocket connection closed for client_id={}, user_id={}",
        client_id, user_id
    );
    state.clients.remove(&client_id);
}

// Function to broadcast a ServerMessage to all connected clients
async fn broadcast_message(message: ServerMessage, state: &AppState) {
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
        message_type(&message),
        state.clients.len()
    );

    // Iterate over all clients and send the message via their channel
    for client_entry in state.clients.iter() {
        let client_id = client_entry.key();
        let client = client_entry.value();

        if client.sender.send(Ok(Message::text(serialized_message.clone()))).is_err() {
            eprintln!("Failed to send broadcast message to client_id={}, user_id={}. Channel likely closed.", client_id, client.user_id);
            // The disconnected client will be cleaned up automatically when its handle_connection task ends.
        }
    }
}

// Helper to get message type for logging
fn message_type(msg: &ServerMessage) -> &'static str {
    match msg {
        ServerMessage::InitialState { .. } => "InitialState",
        ServerMessage::NewPost { .. } => "NewPost",
        ServerMessage::MarketUpdate { .. } => "MarketUpdate",
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
        println!(
            "Received text: client_id={}, user_id={}: {}",
            client_id, user_id, text
        );

        match serde_json::from_str::<ClientMessage>(text) {
            Ok(client_msg) => {
                println!("Deserialized: {:?}", client_msg);

                match client_msg {
                    ClientMessage::CreatePost { content } => {
                        println!("Handling CreatePost from user_id={}: {}", user_id, content);

                        // Create the new post
                        let new_post_id = Uuid::new_v4();
                        let mut new_post = Post {
                            id: new_post_id,
                            user_id: user_id.to_string(),
                            content,
                            timestamp: Utc::now(),
                            supply: 0,
                            price: None, // Price will be calculated before sending
                        };

                        // Store the post
                        state.posts.insert(new_post_id, new_post.clone());
                        println!("Post {} created and stored.", new_post_id);

                         // Calculate price for broadcast
                        new_post.price = Some(get_price(new_post.supply));

                        // Prepare the broadcast message
                        let broadcast_msg = ServerMessage::NewPost { post: new_post };

                        // Broadcast to all clients
                        broadcast_message(broadcast_msg, state).await;
                    }
                    ClientMessage::Buy { post_id } => {
                        println!("Handling Buy for post_id={} from user_id={}", post_id, user_id);
                        // Get the post mutably
                        if let Some(mut post_entry) = state.posts.get_mut(&post_id) {
                            // Update supply
                            post_entry.supply += 1;
                            // Recalculate price
                            let new_price = get_price(post_entry.supply);
                            post_entry.price = Some(new_price);
                            println!(
                                "Post {} updated: supply={}, price={}",
                                post_id, post_entry.supply, new_price
                            );
                            // Prepare broadcast message
                            let update_msg = ServerMessage::MarketUpdate {
                                post_id,
                                price: new_price,
                                supply: post_entry.supply,
                            };
                            // Broadcast the update
                            broadcast_message(update_msg, state).await;
                        } else {
                            eprintln!("Buy error: Post {} not found", post_id);
                            // Optionally send error back to client
                            // send_error_message(client_id, format!("Post {} not found", post_id), state).await;
                        }
                    }
                    ClientMessage::Sell { post_id } => {
                        println!("Handling Sell for post_id={} from user_id={}", post_id, user_id);
                         // Get the post mutably
                        if let Some(mut post_entry) = state.posts.get_mut(&post_id) {
                            // Update supply
                            post_entry.supply -= 1;
                             // Recalculate price
                            let new_price = get_price(post_entry.supply);
                            post_entry.price = Some(new_price);
                             println!(
                                "Post {} updated: supply={}, price={}",
                                post_id, post_entry.supply, new_price
                            );
                            // Prepare broadcast message
                            let update_msg = ServerMessage::MarketUpdate {
                                post_id,
                                price: new_price,
                                supply: post_entry.supply,
                            };
                            // Broadcast the update
                            broadcast_message(update_msg, state).await;
                        } else {
                            eprintln!("Sell error: Post {} not found", post_id);
                            // Optionally send error back to client
                            // send_error_message(client_id, format!("Post {} not found", post_id), state).await;
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "Deserialize error: client_id={}, user_id={}, msg=\"{}\", err={}",
                    client_id, user_id, text, e
                );
                // Send error back to the specific client
                if let Some(client) = state.clients.get(&client_id) {
                    let error_msg = ServerMessage::Error {
                        message: format!("Invalid message format: {}", e),
                    };
                    if let Ok(json_msg) = serde_json::to_string(&error_msg) {
                        if client.sender.send(Ok(Message::text(json_msg))).is_err() {
                            eprintln!(
                                "Failed to send error reply to client_id={}",
                                client_id
                            );
                        }
                    }
                }
            }
        }
    } else if msg.is_ping() {
        println!("Received ping: client_id={}, user_id={}", client_id, user_id);
    } else if msg.is_close() {
        println!("Received close frame: client_id={}, user_id={}", client_id, user_id);
    } else {
         println!("Received non-text: client_id={}, user_id={}", client_id, user_id);
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
        posts: Posts::default(), // Initialize empty posts map
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
