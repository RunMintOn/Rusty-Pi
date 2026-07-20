//! Session tree — in-memory and JSONL-persisted session data structures.
//!
//! Mirrors the original `@earendil-works/pi-agent-core/src/harness/session/` package.

pub mod jsonl;
pub mod memory;
#[allow(clippy::module_inception)]
pub mod session;
pub mod storage;
pub mod types;

// Re-exports of key types for convenience.
pub use jsonl::{JsonlSessionCreateOptions, JsonlSessionStorage};
pub use memory::InMemorySessionStorage;
pub use session::Session;
pub use storage::*;
pub use types::*;
