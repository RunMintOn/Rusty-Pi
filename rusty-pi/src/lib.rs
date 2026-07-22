//! rusty-pi — an independent Rust coding agent library.
//!
//! The modules implement Rusty-Pi's own Agent, Provider, Session, Command,
//! and frontend boundaries. PI is a design reference, not a compatibility
//! specification for this crate.

pub mod agent;
pub mod ai;
pub mod coding_agent;
pub mod format;
pub mod frontends;
pub mod orchestrator;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tui;
