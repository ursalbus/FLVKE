//! Defines the structure of messages exchanged over WebSocket.

use serde::{Deserialize, Serialize};

/// Messages sent from the client to the server.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "payload")] // Use tagged enum representation
pub enum ClientMessage {
    /// Request to create a new post.
    CreatePost { content: String },
    /// Request to place or modify a trade (long/short).
    PlaceTrade {
        post_id: i64, // ID of the post (market) to trade on
        amount: f64,  // The *change* in position amount (+ve for buy/long, -ve for sell/short)
    },
    // Add other client actions here (e.g., GetTimeline, GetMarketDetails)
}

/// Messages sent from the server to the client.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "payload")] // Use tagged enum representation
pub enum ServerMessage {
    /// Confirmation that authentication was successful.
    AuthOk,
    /// Confirmation that a post was successfully created.
    PostCreated { post_id: i64, content: String, creator_id: String },
    /// Confirmation that a trade was placed/updated.
    TradePlaced { post_id: i64, user_id: String, new_position: f64 },
    /// Reports an error back to the client.
    Error { message: String },
    // Add other server messages (e.g., TimelineUpdate, MarketUpdate)
} 