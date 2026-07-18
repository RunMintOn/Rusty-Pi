//! Terminal output formatting layer (Layer 2).
//!
//! Provides [`OutputFormatter`] — a centralized formatter that wraps
//! [`sparcli`] components and produces `String` output suitable for
//! testing via `assert!(contains(...))`.
//!
//! See `tickets/spec-bare-terminal-architecture.md` for the full design.

mod out;

pub use out::{OutputFormatter, SessionInfo, SessionSummary};
