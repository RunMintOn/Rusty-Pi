//! AI provider abstractions and message types.
//!
//! Mirrors the structure of `@earendil-works/pi-ai`.

pub mod auth;
pub mod mock;
pub mod providers;
pub mod stream;
pub mod types;

// Re-exports for convenience.
pub use types::*;
