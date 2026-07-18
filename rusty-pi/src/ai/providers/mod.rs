//! LLM provider implementations.
//!
//! Mirrors `@earendil-works/pi-ai/src/providers/`.

pub mod deepseek;
pub mod openai_codex;

use crate::ai::stream::StreamEvent;
use crate::ai::types::Tool;

/// A registered LLM provider.
#[derive(Debug, Clone)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub models: Vec<Model>,
}

/// A model exposed by a provider.
#[derive(Debug, Clone)]
pub struct Model {
    pub id: &'static str,
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
    ) -> anyhow::Result<StreamReceiver>;

    /// Create a channel for sending stream events.
    fn channel() -> (StreamSender, StreamReceiver) where Self: Sized {
        tokio::sync::mpsc::channel(STREAM_CHANNEL_SIZE)
    }
}
