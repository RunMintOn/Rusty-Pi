//! LLM provider implementations.
//!
//! Mirrors `@earendil-works/pi-ai/src/providers/`.

pub mod deepseek;
pub mod openai_codex;

use crate::ai::stream::StreamEvent;
use crate::ai::types::Tool;

/// A registered LLM provider with its metadata and available models.
#[derive(Debug, Clone)]
pub struct Provider {
    /// Unique provider identifier (e.g. `"deepseek"`, `"openai-codex"`).
    pub id: &'static str,
    /// Human-readable display name.
    pub name: &'static str,
    /// Base URL for the provider's API.
    pub base_url: &'static str,
    /// List of models this provider offers.
    pub models: Vec<Model>,
}

/// A model exposed by a provider.
#[derive(Debug, Clone)]
pub struct Model {
    /// Model identifier, used in API requests (e.g. `"deepseek-v4-pro"`).
    pub id: &'static str,
    /// API family this model uses (e.g. `"openai-completions"`,
    /// `"openai-codex-responses"`).
    pub api: &'static str,
}

/// Size of the stream event channel buffer.
const STREAM_CHANNEL_SIZE: usize = 256;

/// A sender for streaming LLM response events.
pub type StreamSender = tokio::sync::mpsc::Sender<StreamEvent>;

/// A receiver for streaming LLM response events.
pub type StreamReceiver = tokio::sync::mpsc::Receiver<StreamEvent>;

/// A provider that can stream LLM responses.
///
/// Mirrors the stream functions in the original `@earendil-works/pi-ai` package.
/// Instead of returning a single `AssistantMessage`, providers send `StreamEvent`s
/// through a channel and return the receiver.
#[async_trait::async_trait]
pub trait ProviderApi: Send + Sync {
    /// Stream a completion from the LLM.
    /// Returns a receiver that yields `StreamEvent`s.
    async fn stream(
        &self,
        model: &Model,
        messages: &[crate::ai::types::AgentMessage],
        tools: &[&dyn Tool],
        system_prompt: Option<&str>,
    ) -> anyhow::Result<StreamReceiver>;

    /// List models available through this provider.
    ///
    /// The default implementation returns an empty vec; providers that support
    /// runtime model listing should override this.
    fn list_models(&self) -> Vec<&Model> {
        Vec::new()
    }

    /// Create a channel for sending stream events.
    fn channel() -> (StreamSender, StreamReceiver)
    where
        Self: Sized,
    {
        tokio::sync::mpsc::channel(STREAM_CHANNEL_SIZE)
    }
}
