use dashmap::DashMap;
use std::sync::Arc;
use uuid::Uuid;
// use tokio::sync::Mutex; // Removed Mutex import unless needed elsewhere
use std::collections::{VecDeque, BTreeMap};
use ordered_float::OrderedFloat; // For sorting f64 keys

use super::models::{Client, Post, UserPositionDetail};

// Type aliases for shared state
pub type Clients = Arc<DashMap<Uuid, Client>>;         // ClientID -> Client
pub type Posts = Arc<DashMap<Uuid, Post>>;             // PostID -> Post
pub type UserBalances = Arc<DashMap<String, f64>>;   // UserID -> Lifetime Balance (Deposits - Withdrawals)
pub type UserPositions = Arc<DashMap<String, DashMap<Uuid, UserPositionDetail>>>; // UserID -> PostID -> UserPositionDetail
pub type UserRealizedPnl = Arc<DashMap<String, f64>>; // UserID -> Total Realized PNL
pub type UserExposure = Arc<DashMap<String, f64>>;   // UserID -> Cumulative Abs Cost of Open Positions
// pub type LiquidationQueue = Arc<Mutex<VecDeque<String>>>; // Removed

// Map: PostID -> SortedMap[SupplyThreshold -> Vec<(ForcedTradeCost, ForcedTradeSize, UserId)>]
// Use Vec to handle multiple users liquidating at the exact same supply threshold.
pub type LiquidationThresholds = Arc<DashMap<Uuid, BTreeMap<OrderedFloat<f64>, Vec<(f64, f64, String)>>>>;

// pub type InsuranceFund = Arc<DashMap<Uuid, Mutex<f64>>>; // Removed


// Combined Application State
#[derive(Clone)]
pub struct AppState {
    pub clients: Clients,
    pub posts: Posts,
    pub user_balances: UserBalances,
    pub user_positions: UserPositions,
    pub user_realized_pnl: UserRealizedPnl,
    pub user_exposure: UserExposure,
    pub jwt_secret: Arc<String>,
    // pub liquidation_queue: LiquidationQueue, // Removed
    pub liquidation_thresholds: LiquidationThresholds, 
    // pub insurance_fund: InsuranceFund, // Removed
} 