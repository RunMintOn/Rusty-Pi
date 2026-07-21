//! Frontend implementations for the agent.
//!
//! `print` — stateful print frontend for REPL and single-shot modes.
//! All frontends consume the unified `AgentEvent` stream.

pub mod print;

pub use print::PrintFrontend;
