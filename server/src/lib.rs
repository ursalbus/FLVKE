// server/src/lib.rs

// Make modules and types needed for integration tests public.

pub mod protocol;
pub mod bonding_curve;

// Re-export types and functions needed from main.rs logic
// This requires moving the struct/function definitions or making them public in their original module
// Assuming AppState, MarketState, handle_client_message are defined in a module accessible here
// Or we temporarily put the core logic into lib.rs instead of main.rs

// If logic remains in main.rs, we might need to rethink test structure slightly
// or conditionally compile main.rs content as a library.

// For now, let's assume we move/make public the necessary items.
// If main.rs contains the definitions: Add `pub mod main;` and access via `main::AppState` etc.
// Or, better: move struct defs and handlers to separate modules within src/

// ---- Let's refactor main.rs slightly to extract core logic ----
// We will move struct definitions and handlers to new modules.

// Placeholder re-exports assuming refactor:
pub use state::{AppState, MarketState};
pub use handlers::handle_client_message;

pub mod state;
pub mod handlers; 