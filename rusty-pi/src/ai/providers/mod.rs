//! LLM provider implementations.
//!
//! Mirrors `@earendil-works/pi-ai/src/providers/`.

pub mod deepseek;
pub mod openai_codex;

use crate::ai::stream::StreamEvent;
use crate::ai::types::Tool;
use tokio_util::sync::CancellationToken;

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

/// Cancellation and other request-scoped state passed to a provider.
#[derive(Debug, Clone)]
pub struct ProviderRequestContext {
    pub cancellation: CancellationToken,
}

impl ProviderRequestContext {
    pub fn new(cancellation: CancellationToken) -> Self {
        Self { cancellation }
    }
}

/// An owned provider response stream.
///
/// Providers may use a producer task to bridge an HTTP body into the channel,
/// but the task remains owned by this value.  Normal completion and
/// cancellation await it; dropping the stream aborts it as a final fallback.
pub struct ProviderStream {
    receiver: StreamReceiver,
    producer_handle: Option<tokio::task::JoinHandle<()>>,
    cancellation: CancellationToken,
}

impl ProviderStream {
    pub fn new(
        receiver: StreamReceiver,
        producer_handle: tokio::task::JoinHandle<()>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            receiver,
            producer_handle: Some(producer_handle),
            cancellation,
        }
    }

    pub async fn recv(&mut self) -> Option<StreamEvent> {
        self.receiver.recv().await
    }

    /// Await the producer task. This is the ownership proof for a provider
    /// stream that has naturally finished or was cancelled elsewhere.
    pub async fn shutdown(&mut self) {
        self.cancellation.cancel();
        if let Some(handle) = self.producer_handle.take() {
            let _ = handle.await;
        }
    }

    /// Signal request cancellation and await the producer task.
    pub async fn cancel_and_shutdown(&mut self) {
        self.shutdown().await;
    }

    /// Whether the owned producer has completed.
    pub fn producer_finished(&self) -> bool {
        self.producer_handle
            .as_ref()
            .is_none_or(tokio::task::JoinHandle::is_finished)
    }
}

impl Drop for ProviderStream {
    fn drop(&mut self) {
        if let Some(handle) = self.producer_handle.take() {
            self.cancellation.cancel();
            handle.abort();
        }
    }
}

/// A provider that can stream LLM responses.
///
/// Mirrors the stream functions in the original `@earendil-works/pi-ai` package.
/// Instead of returning a single `AssistantMessage`, providers send `StreamEvent`s
/// through an owned channel-backed stream.
#[async_trait::async_trait]
pub trait ProviderApi: Send + Sync {
    /// Stream a completion from the LLM.
    /// Returns an owned stream that yields `StreamEvent`s.
    async fn stream(
        &self,
        model: &Model,
        messages: &[crate::ai::types::AgentMessage],
        tools: &[&dyn Tool],
        system_prompt: Option<&str>,
        context: ProviderRequestContext,
    ) -> anyhow::Result<ProviderStream>;

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
