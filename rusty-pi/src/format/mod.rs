//! Terminal output formatting layer (Layer 2).
//!
//! Provides [`OutputFormatter`] — a centralized formatter that wraps
//! [`sparcli`] components and produces `String` output suitable for
//! testing via `assert!(contains(...))`.
//!
//! The current frontend boundary is documented in `docs/architecture.md` and
//! the capability status in `docs/capabilities.md`. Earlier bare-terminal
//! planning is retained under `tickets/` as historical material.

mod out;

pub use out::{OutputFormatter, SessionInfo, SessionSummary};
