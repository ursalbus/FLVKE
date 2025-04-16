use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;
use warp::filters::ws::Message;

// --- JWT & Auth Types ---

// Represents the claims expected in the Supabase JWT
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // Subject (user ID)
    pub aud: String, // Audience
    pub exp: usize,  // Expiration time
}

// Structure to deserialize the query parameter containing the token
#[derive(Deserialize, Debug)]
pub struct AuthQuery {
    pub token: String,
}

// --- Core Data Models ---

// Represents a post in the timeline
#[derive(Debug, Serialize, Clone)]
pub struct Post {
    pub id: Uuid,
    pub user_id: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    pub supply: f64,
}

// Holds the details of a user's position in a specific post
#[derive(Debug, Clone, Default)]
pub struct UserPositionDetail {
    pub size: f64,
    pub total_cost_basis: f64,
}

// Structure to hold client-specific information
#[derive(Debug)]
pub struct Client {
    pub user_id: String,
    pub sender: UnboundedSender<Result<Message, warp::Error>>,
}

// --- WebSocket Message Types ---

// Represents incoming messages from the client
#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    CreatePost { content: String },
    Buy { post_id: Uuid, quantity: f64 },
    Sell { post_id: Uuid, quantity: f64 },
}

// Used within UserSync to send position details
#[derive(Serialize, Debug, Clone)]
pub struct PositionDetail {
    pub post_id: Uuid,
    pub size: f64,
    pub average_price: f64,
    pub unrealized_pnl: f64,
}

// Represents messages sent from the server to the client
#[derive(Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    InitialState { posts: Vec<Post> },
    UserSync {
        balance: f64,
        exposure: f64,
        equity: f64,
        positions: Vec<PositionDetail>,
        total_realized_pnl: f64,
    },
    NewPost { post: Post },
    MarketUpdate { post_id: Uuid, price: f64, supply: f64 },
    BalanceUpdate { balance: f64 },
    PositionUpdate {
        post_id: Uuid,
        size: f64,
        average_price: f64,
        unrealized_pnl: f64,
    },
    RealizedPnlUpdate { total_realized_pnl: f64 },
    ExposureUpdate { exposure: f64 },
    EquityUpdate { equity: f64 },
    Error { message: String },
} 