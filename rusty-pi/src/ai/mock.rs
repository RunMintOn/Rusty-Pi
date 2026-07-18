//! Mock provider for testing.

use crate::ai::providers::{Model, ProviderApi, StreamReceiver};
use crate::ai::stream::{MessageAccumulator, StreamEvent};
use crate::ai::types::{AgentMessage, StopReason, Tool};
use async_trait::async_trait;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub enum MockStep {
    Text(String),
    ToolCall { id: String, name: String, arguments: serde_json::Value },
    Error(String),
}

pub struct MockProvider {
    steps: Mutex<Vec<MockStep>>,
}

impl MockProvider {
    pub fn new(steps: Vec<MockStep>) -> Self {
        Self { steps: Mutex::new(steps) }
    }

    pub fn text(text: &str) -> Self {
        Self::new(vec![MockStep::Text(text.to_string())])
    }
}

#[async_trait]
impl ProviderApi for MockProvider {
    async fn stream(
        &self,
        _model: &Model,
        _messages: &[AgentMessage],
        _tools: &[&dyn Tool],
    ) -> anyhow::Result<StreamReceiver> {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let mut steps = self.steps.lock().unwrap();
        let step = steps.first().cloned();
        if steps.len() > 1 {
            steps.remove(0);
        }
        let step = step.unwrap_or(MockStep::Text("(done)".into()));

        tokio::spawn(async move {
            match step {
                MockStep::Text(text) => {
                    for word in text.split(' ') {
                        let chunk = format!("{} ", word);
                        if tx.send(StreamEvent::TextDelta { delta: chunk }).await.is_err() { return; }
                    }
                    let acc = MessageAccumulator::new("mock", "mock", "mock");
                    let msg = acc.build();
                    let _ = tx.send(StreamEvent::Done { message: msg }).await;
                }
                MockStep::ToolCall { id, name, arguments } => {
                    let _ = tx.send(StreamEvent::ToolCall { id, name: name.clone(), arguments: arguments.clone() }).await;
                    let acc = MessageAccumulator::new("mock", "mock", "mock");
                    let msg = acc.build();
                    let _ = tx.send(StreamEvent::Done { message: msg }).await;
                }
                MockStep::Error(msg) => {
                    let _ = tx.send(StreamEvent::Error { reason: StopReason::Error, message: msg }).await;
                }
            }
        });

        Ok(rx)
    }
}
