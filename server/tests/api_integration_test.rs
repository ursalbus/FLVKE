// server/tests/api_integration_test.rs
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sqlx::{Connection, Executor, PgConnection, PgPool};
use tokio::sync::Mutex;
use uuid::Uuid;

// Import items from the main crate (server)
use fluke_server::{
    protocol::{ClientMessage, ServerMessage},
    AppState, // Assuming AppState is made public or accessible for tests
    handle_client_message, // Assuming this is made public or accessible
    MarketState,
    // We need access to the handler functions directly or simulate AppState
    // Let's assume AppState and handle_client_message are available.
    // If not, we might need to adjust visibility or test setup.
};
use postgrest::Postgrest; // Needed to construct AppState

// Test Database Configuration
async fn configure_test_db() -> PgPool {
    // Load .env or .env.test variables
    dotenvy::dotenv().ok();
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for integration tests");

    // Create connection pool
    let pool = PgPool::connect(&database_url)
        .await
        .expect("Failed to create pool.");

    // Optional: Run migrations here using sqlx::migrate! if needed,
    // but assume migrations are applied separately for now.

    pool
}

/// Helper to clean up test data
async fn cleanup_db(pool: &PgPool, user_ids: Vec<Uuid>) {
    if user_ids.is_empty() {
        return;
    }

    // Use explicit UUID array type for binding
    let user_ids_pg: Vec<Uuid> = user_ids.clone(); // Ensure it's Vec<Uuid>

    // Delete positions
    sqlx::query!("DELETE FROM public.positions WHERE user_id = ANY($1::uuid[])", &user_ids_pg)
        .execute(pool)
        .await
        .expect("Failed to delete test positions");

    // Delete posts
    sqlx::query!("DELETE FROM public.posts WHERE user_id = ANY($1::uuid[])", &user_ids_pg)
        .execute(pool)
        .await
        .expect("Failed to delete test posts");

    // Delete auth users (cascade should handle profiles)
    // IMPORTANT: Only do this in a dedicated test environment!
    sqlx::query!("DELETE FROM auth.users WHERE id = ANY($1::uuid[])", &user_ids_pg)
        .execute(pool)
        .await
        .expect("Failed to delete test auth users");

    println!("Cleaned up test data for users: {:?}", user_ids);
}

/// Helper to create a test user directly in the database with a unique email
async fn create_test_user(pool: &PgPool, username_base: &str, initial_balance: Decimal) -> Uuid {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let unique_username = format!("{}_{}", username_base, timestamp);
    let unique_email = format!("{}@test.com", unique_username);

    let user_id = Uuid::new_v4();
    // Minimal insert into auth.users
    sqlx::query!(
        "INSERT INTO auth.users (id, email, raw_user_meta_data) VALUES ($1, $2, $3)",
        user_id,
        unique_email, // Use the unique email
        // Store the unique username in metadata so the trigger can pick it up
        serde_json::json!({ "username": unique_username })
    )
    .execute(pool)
    .await
    .expect("Failed to insert test auth user");

    // The handle_new_user trigger should create the profile with the unique username.
    // Manually update the balance.
    sqlx::query!(
        "UPDATE public.user_profiles SET balance = $1 WHERE id = $2",
        initial_balance,
        user_id
    )
    .execute(pool)
    .await
    .expect("Failed to set initial balance");

    user_id
}

// --- Test Cases ---

#[tokio::test]
async fn test_create_post_success() {
    let pool = configure_test_db().await;
    let user_id = create_test_user(&pool, "test_user_create", dec!(1000.0)).await;
    let user_ids_to_clean = vec![user_id];

    // Simulate AppState using SERVICE KEY via Authorization header
    let supabase_url = std::env::var("SUPABASE_URL").expect("SUPABASE_URL missing");
    let service_key = std::env::var("SUPABASE_SERVICE_KEY").expect("SUPABASE_SERVICE_KEY missing for tests");
    let rest_url = format!("{}/rest/v1", supabase_url);
    // Use Authorization header for service key
    let auth_header_value = format!("Bearer {}", service_key);
    let db_client = Arc::new(
        Postgrest::new(rest_url)
            .insert_header("apikey", &service_key) // Still need apikey
            .insert_header("Authorization", auth_header_value) // Add Bearer token
    );
    let market_state: MarketState = Arc::new(Mutex::new(HashMap::new()));
    let app_state = AppState {
        db_client,
        auth_clients: Arc::new(Mutex::new(HashMap::new())),
        pending_clients: Arc::new(Mutex::new(HashMap::new())),
        market_state,
    };

    let client_msg = ClientMessage::CreatePost { content: "Test Post Content".to_string() };
    let addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();

    let response = handle_client_message(
        app_state.db_client.clone(),
        app_state.market_state.clone(),
        user_id.to_string(),
        client_msg,
        addr,
    ).await;

    // Assert response
    let post_id = match response {
        ServerMessage::PostCreated { post_id, content, creator_id } => {
            assert_eq!(content, "Test Post Content");
            assert_eq!(creator_id, user_id.to_string());
            post_id // Return post_id for db verification
        }
        _ => panic!("Expected PostCreated, got {:?}", response),
    };

    // Assert database state
    let db_post = sqlx::query!("SELECT content, user_id FROM public.posts WHERE id = $1", post_id)
        .fetch_one(&pool)
        .await
        .expect("Post not found in DB");

    assert_eq!(db_post.content.unwrap(), "Test Post Content");
    assert_eq!(db_post.user_id, user_id);

    cleanup_db(&pool, user_ids_to_clean).await; // Pass user IDs to cleanup
}

#[tokio::test]
async fn test_place_trade_buy_success() {
    let pool = configure_test_db().await;
    let user_id = create_test_user(&pool, "test_user_buy", dec!(20.0)).await;
    let user_ids_to_clean = vec![user_id];

    // Manually create a post for trading
    let post_id: i64 = sqlx::query!(
        "INSERT INTO public.posts (user_id, content) VALUES ($1, $2) RETURNING id",
        user_id,
        "Trade Test Post"
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to insert test post")
    .id;

    // Simulate AppState using SERVICE KEY via Authorization header
    let supabase_url = std::env::var("SUPABASE_URL").expect("SUPABASE_URL missing");
    let service_key = std::env::var("SUPABASE_SERVICE_KEY").expect("SUPABASE_SERVICE_KEY missing for tests");
    let rest_url = format!("{}/rest/v1", supabase_url);
    let auth_header_value = format!("Bearer {}", service_key);
    let db_client = Arc::new(
        Postgrest::new(rest_url)
            .insert_header("apikey", &service_key) // Still need apikey
            .insert_header("Authorization", auth_header_value) // Add Bearer token
    );
    let market_state: MarketState = Arc::new(Mutex::new(HashMap::new())); // Starts empty
    let app_state = AppState {
        db_client,
        auth_clients: Arc::new(Mutex::new(HashMap::new())), // Dummy
        pending_clients: Arc::new(Mutex::new(HashMap::new())), // Dummy
        market_state: market_state.clone(), // Clone Arc
    };

    let client_msg = ClientMessage::PlaceTrade { post_id, amount: 1.0 }; // Buy 1 share
    let addr: SocketAddr = "127.0.0.1:12346".parse().unwrap();

    let response = handle_client_message(
        app_state.db_client.clone(),
        app_state.market_state.clone(),
        user_id.to_string(),
        client_msg,
        addr,
    ).await;

    // Assert response
    match response {
        ServerMessage::TradePlaced { post_id: resp_post_id, user_id: resp_user_id, new_position } => {
            assert_eq!(resp_post_id, post_id);
            assert_eq!(resp_user_id, user_id.to_string());
            assert!((new_position - 1.0).abs() < 1e-9); // Position should be 1.0
        }
        _ => panic!("Expected TradePlaced, got {:?}", response),
    }

    // Assert database state
    // Expected cost I(1) - I(0) = 5/3 = 1.666...
    let expected_cost = dec!(1.6666666666666667); // Use precise decimal
    let expected_balance = dec!(20.0) - expected_cost;
    let db_balance = sqlx::query_scalar!("SELECT balance FROM public.user_profiles WHERE id = $1", user_id)
        .fetch_one(&pool)
        .await
        .expect("User profile not found");
    assert!((db_balance - expected_balance).abs() < dec!(0.00000001)); // Check balance

    let db_position = sqlx::query_scalar!("SELECT amount FROM public.positions WHERE user_id = $1 AND post_id = $2", user_id, post_id)
        .fetch_one(&pool)
        .await
        .expect("Position not found");
    assert_eq!(db_position, dec!(1.0)); // Check position amount

    // Assert in-memory state
    let final_supply = *market_state.lock().await.get(&post_id).unwrap();
    assert!((final_supply - 1.0).abs() < 1e-9); // Supply should be 1.0

    cleanup_db(&pool, user_ids_to_clean).await;
}


#[tokio::test]
async fn test_place_trade_insufficient_balance() {
     let pool = configure_test_db().await;
    let user_id = create_test_user(&pool, "test_user_nobal", dec!(1.0)).await;
    let user_ids_to_clean = vec![user_id];

    // Start with very low balance
    let post_id: i64 = sqlx::query!(
        "INSERT INTO public.posts (user_id, content) VALUES ($1, $2) RETURNING id",
        user_id,
        "No Balance Trade Post"
    )
    .fetch_one(&pool)
    .await.unwrap().id;

    // Simulate AppState using SERVICE KEY via Authorization header
    let supabase_url = std::env::var("SUPABASE_URL").expect("SUPABASE_URL missing");
    let service_key = std::env::var("SUPABASE_SERVICE_KEY").expect("SUPABASE_SERVICE_KEY missing for tests");
    let rest_url = format!("{}/rest/v1", supabase_url);
    let auth_header_value = format!("Bearer {}", service_key);
    let db_client = Arc::new(
        Postgrest::new(rest_url)
            .insert_header("apikey", &service_key) // Still need apikey
            .insert_header("Authorization", auth_header_value) // Add Bearer token
    );
    let market_state: MarketState = Arc::new(Mutex::new(HashMap::new()));
    let app_state = AppState {
        db_client,
        auth_clients: Arc::new(Mutex::new(HashMap::new())),
        pending_clients: Arc::new(Mutex::new(HashMap::new())),
        market_state: market_state.clone(),
    };

    let client_msg = ClientMessage::PlaceTrade { post_id, amount: 1.0 }; // Try to buy 1 share (costs ~1.67)
    let addr: SocketAddr = "127.0.0.1:12347".parse().unwrap();

    let response = handle_client_message(
        app_state.db_client.clone(),
        app_state.market_state.clone(),
        user_id.to_string(),
        client_msg,
        addr,
    ).await;

    // Assert response is Error::InsufficientBalance
    match response {
        ServerMessage::Error { message } => {
            assert!(message.contains("Insufficient balance"));
        }
        _ => panic!("Expected Insufficient balance Error, got {:?}", response),
    }

     // Assert database state hasn't changed (balance, position)
     let db_balance = sqlx::query_scalar!("SELECT balance FROM public.user_profiles WHERE id = $1", user_id)
        .fetch_one(&pool).await.unwrap();
    assert_eq!(db_balance, dec!(1.0)); // Balance should remain 1.0

    // Check if a position exists using fetch_optional
    let position_row = sqlx::query!("SELECT post_id FROM public.positions WHERE user_id = $1 AND post_id = $2", user_id, post_id)
        .fetch_optional(&pool)
        .await
        .expect("DB error checking position existence");
    assert!(position_row.is_none(), "Position should not have been created"); // Assert that no row was found

    // Assert in-memory state hasn't changed
    let supply = market_state.lock().await.get(&post_id).copied().unwrap_or(0.0);
    assert!(supply.abs() < 1e-9); // Supply should remain 0.0

    cleanup_db(&pool, user_ids_to_clean).await;
}

// TODO: Add more tests:
// - test_place_trade_sell_success
// - test_place_trade_sell_insufficient_shares
// - test_place_trade_buy_then_sell
// - test_create_post_duplicate_content (if constraints added)
// - test database error handling (e.g., simulate DB down) 