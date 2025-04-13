use dashmap::DashMap;
use std::sync::Arc;
use uuid::Uuid;

use super::models::{Client, Post, UserPositionDetail};

// Type aliases for shared state
pub type Clients = Arc<DashMap<Uuid, Client>>;         // ClientID -> Client
pub type Posts = Arc<DashMap<Uuid, Post>>;             // PostID -> Post
pub type UserBalances = Arc<DashMap<String, f64>>;   // UserID -> Balance
pub type UserPositions = Arc<DashMap<String, DashMap<Uuid, UserPositionDetail>>>; // UserID -> PostID -> UserPositionDetail
pub type UserRealizedPnl = Arc<DashMap<String, f64>>; // UserID -> Total Realized PNL

// Combined Application State
#[derive(Clone)]
pub struct AppState {
    pub clients: Clients,
    pub posts: Posts,
    pub user_balances: UserBalances,
    pub user_positions: UserPositions,
    pub user_realized_pnl: UserRealizedPnl,
    pub jwt_secret: Arc<String>,
} 