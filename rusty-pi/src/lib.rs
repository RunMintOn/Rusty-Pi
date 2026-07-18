//! rusty-pi — Rust rewrite of the pi coding agent.
//!
//! Module structure mirrors the original TypeScript monorepo packages:
//! - `ai` → `@earendil-works/pi-ai`
//! - `agent` → `@earendil-works/pi-agent-core`
//! - `coding_agent` → `@earendil-works/pi-coding-agent`
//! - `tui` → `@earendil-works/pi-tui`
//! - `orchestrator` → `@earendil-works/pi-orchestrator`

pub mod agent;
pub mod ai;
pub mod coding_agent;
pub mod format;
pub mod orchestrator;
pub mod tui;
