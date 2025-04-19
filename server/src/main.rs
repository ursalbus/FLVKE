// Declare modules
mod auth;
mod bonding_curve;
mod calculations;
mod constants;
mod errors;
mod handlers;
mod models;
mod state;
mod websocket;

use dotenvy::dotenv;
use std::{env, sync::Arc};
use warp::{
    http::StatusCode,
    Filter,
};

// Use items from modules
use auth::with_auth;
use errors::handle_rejection;
use state::{AppState, Clients, Posts, UserBalances, UserPositions, UserRealizedPnl, UserExposure, LiquidationThresholds};
use websocket::handle_connection;

#[tokio::main]
async fn main() {
     if dotenvy::from_filename("../.env").is_err() {
         if dotenv().is_err() {
             eprintln!("Warning: .env file not found.");
         }
     }

    let jwt_secret = env::var("JTW_SECRET").expect("JTW_SECRET must be set in .env file");

    // Initialize shared state using types defined in state.rs
    let app_state = AppState {
        clients: Clients::default(),
        posts: Posts::default(),
        user_balances: UserBalances::default(),
        user_positions: UserPositions::default(),
        user_realized_pnl: UserRealizedPnl::default(),
        user_exposure: UserExposure::default(),
        jwt_secret: Arc::new(jwt_secret),
        liquidation_thresholds: LiquidationThresholds::default(),
    };

    println!("JWT Secret loaded.");

    // Define routes using functions from modules
    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(with_auth(app_state.clone())) // from auth.rs
        .and(warp::any().map(move || app_state.clone()))
        .map(|ws: warp::ws::Ws, user_id: String, state: AppState| {
            ws.on_upgrade(move |websocket| handle_connection(websocket, user_id, state)) // from websocket.rs
        });

    let health_route = warp::path!("health").map(|| StatusCode::OK);

    let routes = health_route.or(ws_route).recover(handle_rejection); // from errors.rs

    let addr = "127.0.0.1:8080";
    println!("Server starting on {}", addr);

    warp::serve(routes)
        .run(addr.parse::<std::net::SocketAddr>().unwrap())
        .await;
}

