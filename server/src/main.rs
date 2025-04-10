// server/src/main.rs
// Main binary entry point.
// Imports logic from the library modules (state, handlers, etc.)

use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use dotenvy::dotenv;
use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use postgrest::Postgrest;
use serde_json;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio::time::timeout;
use tokio_tungstenite::{
    accept_async,
    tungstenite::{protocol::Message, Error as WsError},
};

// Import necessary items from our library crate `fluke_server`
use fluke_server::{
    handlers::handle_client_message,
    protocol::{ClientMessage, ServerMessage},
    state::{AppState, AuthenticatedClientMap, Claims, MarketState, PendingClientMap},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    // --- Configuration --- //
    let supabase_url = env::var("SUPABASE_URL").expect("SUPABASE_URL must be set");
    let supabase_anon_key = env::var("SUPABASE_ANON_KEY").expect("SUPABASE_ANON_KEY must be set");
    let jwt_secret = env::var("SUPABASE_JWT_SECRET").expect("SUPABASE_JWT_SECRET must be set");
    let jwt_decoding_key = DecodingKey::from_secret(jwt_secret.as_ref());
    let addr = env::var("LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    // --- Initialization --- //
    // Construct the full PostgREST endpoint URL
    let rest_url = format!("{}/rest/v1", supabase_url);
    let db_client = Arc::new(Postgrest::new(rest_url).insert_header("apikey", supabase_anon_key));
    println!("Database client initialized.");

    let listener = TcpListener::bind(&addr).await?;
    println!("WebSocket server listening on: {}", addr);

    // Initialize shared state using types imported from the library
    let market_state: MarketState = Arc::new(Mutex::new(HashMap::new()));
    let auth_clients: AuthenticatedClientMap = Arc::new(Mutex::new(HashMap::new()));
    let pending_clients: PendingClientMap = Arc::new(Mutex::new(HashMap::new()));

    // TODO: Load initial market state from DB?

    let app_state = AppState {
        db_client,
        auth_clients,
        pending_clients,
        market_state,
    };

    // --- Server Loop --- //
    while let Ok((stream, addr)) = listener.accept().await {
        // Clone Arcs for the connection handler (cheap)
        let state_clone = app_state.clone();
        let jwt_decoding_key_clone = jwt_decoding_key.clone();
        tokio::spawn(handle_connection(
            stream,
            addr,
            state_clone,
            jwt_decoding_key_clone,
        ));
    }
    Ok(())
}

// --- Connection Handling Logic --- //

const AUTH_TIMEOUT: Duration = Duration::from_secs(5);

/// Handles a single WebSocket connection.
async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    state: AppState,
    jwt_decoding_key: DecodingKey,
) {
    println!("[{}] Incoming TCP connection.", addr);

    // WebSocket Handshake
    let ws_stream = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            println!("[{}] WebSocket handshake error: {}", addr, e);
            return;
        }
    };
    println!("[{}] WebSocket connection established. Waiting for auth...", addr);

    let (mut writer, mut reader) = ws_stream.split();

    // Channel for message passing between read/write/handler tasks
    let (tx, mut rx) = mpsc::channel::<Message>(32);
    state.pending_clients.lock().await.insert(addr, tx.clone());

    // Spawn task to send messages from channel to client
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if writer.send(msg).await.is_err() {
                println!("[{}] Send task: Error sending message. Closing.", addr);
                break;
            }
        }
        println!("[{}] Send task finished.", addr);
    });

    // --- Authentication Step --- //
    let auth_result: Result<String, String> = async {
        match timeout(AUTH_TIMEOUT, reader.next()).await {
            Ok(Some(Ok(Message::Text(token)))) => {
                let mut validation = Validation::new(Algorithm::HS256);
                validation.validate_aud = false;
                match decode::<Claims>(&token, &jwt_decoding_key, &validation) { // Use Claims from state
                    Ok(token_data) => {
                        let user_id = token_data.claims.sub;
                        println!("[{}] JWT validation successful for user: {}", addr, user_id);
                        Ok(user_id)
                    }
                    Err(e) => {
                        println!("[{}] JWT validation failed: {:?}", addr, e);
                        Err(format!("JWT validation failed: {}", e))
                    }
                }
            }
            Ok(Some(Ok(_))) => Err("Invalid auth message type".to_string()),
            Ok(Some(Err(e))) => Err(format!("Error receiving auth message: {}", e)),
            Ok(None) | Err(_) => Err("Authentication timed out or connection closed".to_string()),
        }
    }
    .await;

    // --- Post-Authentication Handling --- //
    let user_id = match auth_result {
        Ok(id) => {
            // Move connection from pending map to authenticated map
            if let Some(sender) = state.pending_clients.lock().await.remove(&addr) {
                state.auth_clients.lock().await.insert(addr, (id.clone(), sender));
                println!("[{}] Client authenticated for user {}.", addr, id);

                // Send confirmation message to client
                let auth_ok_msg = ServerMessage::AuthOk;
                if let Ok(json_msg) = serde_json::to_string(&auth_ok_msg) {
                    if tx.send(Message::Text(json_msg)).await.is_err() {
                        println!("[{}] Failed to send auth_ok message (client likely disconnected).", addr);
                    }
                } else {
                    println!("[{}] CRITICAL: Failed to serialize AuthOk message.", addr);
                }
                id // Return user_id
            } else {
                println!("[{}] CRITICAL: Authenticated user not found in pending map.", addr);
                let _ = tx.send(Message::Close(None)).await; // Signal send_task to close
                return; // Terminate connection handler
            }
        }
        Err(reason) => {
            // Authentication failed
            println!("[{}] Authentication failed: {}. Closing connection.", addr, reason);
            state.pending_clients.lock().await.remove(&addr);
            let close_msg = Message::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Policy,
                reason: std::borrow::Cow::Owned(format!("Authentication failed: {}", reason)),
            }));
            let _ = tx.send(close_msg).await; // Attempt to notify client
            return;
        }
    };

    // --- Authenticated Message Handling Loop --- //
    println!("[{}] Starting message loop for user: {}", addr, user_id);
    let db_client = state.db_client.clone();
    let auth_clients_clone = state.auth_clients.clone();
    let market_state_clone = state.market_state.clone();
    // Clone user_id HERE before it's moved into receive_task
    let user_id_clone = user_id.clone();

    // Task to read incoming messages from the authenticated client
    let receive_task = async move {
        // user_id is moved into this block implicitly
        while let Some(msg_result) = reader.next().await {
            let client_tx = { // Get sender clone
                let clients_guard = auth_clients_clone.lock().await;
                clients_guard.get(&addr).map(|(_, sender)| sender.clone())
            };

            let client_tx = match client_tx {
                Some(tx) => tx,
                None => {
                    println!("[{}] Receive loop: Client disconnected (sender lost).", addr);
                    break;
                }
            };

            match msg_result {
                Ok(Message::Text(text)) => {
                    match serde_json::from_str::<ClientMessage>(&text) {
                        Ok(client_message) => {
                            let handler_db_client = db_client.clone();
                            let handler_market_state = market_state_clone.clone();
                            // Use the already moved user_id here, or clone the clone if needed inside spawn
                            let handler_user_id = user_id.clone();
                            let handler_tx = client_tx.clone();

                            tokio::spawn(async move {
                                let response_msg = handle_client_message(
                                    handler_db_client,
                                    handler_market_state,
                                    handler_user_id,
                                    client_message,
                                    addr,
                                )
                                .await;
                                // Send the response from the handler back to the client
                                if let Ok(json_response) = serde_json::to_string(&response_msg) {
                                    if handler_tx.send(Message::Text(json_response)).await.is_err()
                                    {
                                        println!("[{}] Handler task: Error sending response.", addr);
                                    }
                                } else {
                                    println!("[{}] CRITICAL: Failed to serialize handler response: {:?}", addr, response_msg);
                                    let err_resp = ServerMessage::Error {
                                        message: "Internal server error (serialization)".to_string(),
                                    };
                                    if let Ok(json_err) = serde_json::to_string(&err_resp) {
                                        let _ = handler_tx.send(Message::Text(json_err)).await;
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            // Failed to parse client message
                            println!(
                                "[{}] Failed to parse message from user {}: {}. Text: '{}'",
                                addr, user_id, e, text
                            );
                            let err_response = ServerMessage::Error {
                                message: format!("Invalid message format: {}", e),
                            };
                            if let Ok(json_err) = serde_json::to_string(&err_response) {
                                if client_tx.send(Message::Text(json_err)).await.is_err() {
                                    println!("[{}] Error sending parse error response.", addr);
                                }
                            } else {
                                println!(
                                    "[{}] CRITICAL: Failed to serialize parse error response.",
                                    addr
                                );
                            }
                        }
                    }
                }
                // Handle other WebSocket message types
                Ok(Message::Binary(_)) => {
                    println!("[{}] Received binary message (ignored).", addr);
                }
                Ok(Message::Ping(_)) => {
                    println!("[{}] Received Ping (Pong handled automatically).", addr);
                }
                Ok(Message::Pong(_)) => {
                    println!("[{}] Received Pong.", addr);
                }
                Ok(Message::Close(_)) => {
                    println!("[{}] Received Close frame.", addr);
                    break; // Exit receive loop
                }
                Ok(Message::Frame(_)) => {
                    // Usually ignore raw frames
                }
                // Handle WebSocket errors
                Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => {
                    println!("[{}] Connection closed gracefully.", addr);
                    break; // Exit receive loop
                }
                Err(e) => {
                    println!("[{}] WebSocket receive error: {}", addr, e);
                    break; // Exit loop on error
                }
            }
        }
        println!("[{}] Receive loop finished for user {}.", addr, user_id);
    };

    // --- Cleanup --- //
    // Use the clone created *before* receive_task
    let user_id_for_cleanup = user_id_clone;

    tokio::select! {
        _ = send_task => println!("[{}] Send task exited.", addr),
        _ = receive_task => println!("[{}] Receive task exited.", addr),
    }

    println!(
        "[{}] Cleaning up connection state for user {}.",
        addr, user_id_for_cleanup
    );
    // Remove client from the authenticated map
    state.auth_clients.lock().await.remove(&addr);
    println!("[{}] Connection fully closed for user {}.", addr, user_id_for_cleanup);
}
