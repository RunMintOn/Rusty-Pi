//! Session tree — in-memory and JSONL-persisted session data structures.
//!
//! Mirrors the original `@earendil-works/pi-agent-core/src/harness/session/` package.

pub mod types;
pub mod storage;
pub mod memory;
pub mod jsonl;
#[allow(clippy::module_inception)]
pub mod session;

// Re-exports of key types for convenience.
pub use types::*;
pub use storage::*;
pub use memory::InMemorySessionStorage;
pub use jsonl::{JsonlSessionStorage, JsonlSessionCreateOptions};
pub use session::Session;
