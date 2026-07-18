//! AI provider abstractions and message types.
//!
//! Mirrors the structure of `@earendil-works/pi-ai`.

pub mod types;
pub mod providers;
pub mod mock;
pub mod stream;

// Re-exports for convenience.
pub use types::*;
