//! LLM provider implementations.
//!
//! Mirrors `@earendil-works/pi-ai/src/providers/`.
//!
//! Each provider module exports a factory function that returns a configured provider.
//! Providers are registered in the provider registry and discovered at startup via
//! environment variables or configuration.

pub mod deepseek;
pub mod openai_codex;

use crate::ai::types::{AgentMessage, AssistantMessage, Tool};
use async_trait::async_trait;

/// A registered LLM provider.
#[derive(Clone)]
pub struct Provider {
    /// Provider identifier (e.g., "deepseek", "openai-codex").
    pub id: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Base URL for API requests.
    pub base_url: &'static str,
    /// Available models for this provider.
    pub models: Vec<Model>,
}

/// A model exposed by a provider.
#[derive(Debug, Clone)]
pub struct Model {
    /// Model identifier (e.g., "deepseek-v4-pro").
    pub id: &'static str,
    /// API type used to call this model.
    pub api: &'static str,
}

/// A provider that can stream LLM responses.
///
/// Mirrors the stream functions in the original `@earendil-works/pi-ai` package.
/// Each API implementation (e.g., openai-completions, openai-codex-responses)
/// implements this trait.
#[async_trait]
pub trait ProviderApi: Send + Sync {
    /// Stream a completion from the LLM.
    async fn stream(
        &self,
        model: &Model,
        messages: &[AgentMessage],
        tools: &[&dyn Tool],
    ) -> anyhow::Result<AssistantMessage>;
}
