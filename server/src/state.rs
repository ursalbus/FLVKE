// server/src/state.rs
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::protocol::Message;
use postgrest::Postgrest;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

// --- Shared Types --- (Moved from main.rs)

// Type to store authenticated client info (user_id associated with their message sender)
pub type AuthenticatedClientMap = Arc<Mutex<HashMap<SocketAddr, (String, mpsc::Sender<Message>)>>>;

// Type for pending connections that haven't authenticated yet
pub type PendingClientMap = Arc<Mutex<HashMap<SocketAddr, mpsc::Sender<Message>>>>;

// Map to hold market supply (Post ID -> Supply)
pub type MarketState = Arc<Mutex<HashMap<i64, f64>>>;

// Structure to hold shared application state
#[derive(Clone)]
pub struct AppState {
    pub db_client: Arc<Postgrest>,
    pub auth_clients: AuthenticatedClientMap,
    pub pending_clients: PendingClientMap,
    pub market_state: MarketState,
}

// --- Database Structs --- (Moved from main.rs)

#[derive(Serialize, Debug)]
pub struct NewPost<'a> {
    pub user_id: &'a str,
    pub content: &'a str,
}

#[derive(Deserialize, Debug)]
pub struct Post {
    pub id: i64,
    pub user_id: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserProfile {
    pub id: String,
    #[serde(with = "rust_decimal::serde::float")]
    pub balance: Decimal,
    pub username: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Position {
    pub id: Option<i64>,
    pub user_id: String,
    pub post_id: i64,
    #[serde(with = "rust_decimal::serde::float")]
    pub amount: Decimal,
    pub updated_at: String,
}

// --- Auth Structs --- (Moved from main.rs)

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
   pub sub: String,
   pub exp: usize,
} 