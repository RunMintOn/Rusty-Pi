//! Tests for AgentEvent emission sequences.

#[cfg(test)]
mod tests {
    use crate::agent::engine::Agent;
    use crate::agent::events::AgentEvent;
    use crate::agent::types::{AgentTool, AgentToolResult};
    use crate::ai::mock::{MockProvider, MockStep, MultiToolCallProvider};
    use crate::ai::providers::Model;
    use crate::ai::types::{Content, StopReason, Tool};
    use async_trait::async_trait;
    use std::sync::Arc;

    fn make_model() -> Model {
        Model {
            id: "mock",
            api: "mock",
        }
    }

    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes input"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            })
        }
    }

    #[async_trait]
    impl AgentTool for EchoTool {
        fn label(&self) -> &str {
            "echo"
        }
        async fn execute(
            &self,
            _tool_call_id: &str,
            params: serde_json::Value,
            _signal: Option<tokio::sync::watch::Receiver<bool>>,
        ) -> anyhow::Result<AgentToolResult> {
            let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
            Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: format!("echo: {}", text),
                }],
                ..Default::default()
            })
        }
    }

    struct FailTool;

    impl Tool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
    }

    #[async_trait]
    impl AgentTool for FailTool {
        fn label(&self) -> &str {
            "fail"
        }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _signal: Option<tokio::sync::watch::Receiver<bool>>,
        ) -> anyhow::Result<AgentToolResult> {
            Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: "tool error occurred".into(),
                }],
                is_error: true,
                exit_code: Some(1),
                ..Default::default()
            })
        }
    }

    /// Helper: collect all events from an agent run.
    /// Takes ownership of the sender so it can be dropped after run completes.
    async fn collect_events(agent: &mut Agent, prompt: &str) -> Vec<AgentEvent> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        agent.set_event_sender(tx);
        agent.run(prompt).await.unwrap();
        // tx is still in agent, drop it by setting a new dummy
        let (dummy_tx, _dummy_rx) = tokio::sync::mpsc::channel(1);
        agent.set_event_sender(dummy_tx);
        // Now rx should close
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    // ── Text response event sequence ──

    #[tokio::test]
    async fn text_response_events_in_order() {
        let mock = MockProvider::text("Hello from mock");
        let mut agent = Agent::new(Box::new(mock), make_model());
        let events = collect_events(&mut agent, "Hi").await;

        assert!(!events.is_empty());
        assert!(matches!(&events[0], AgentEvent::RunStarted));

        // Should have at least one TextDelta
        let text_deltas: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TextDelta { .. }))
            .collect();
        assert!(!text_deltas.is_empty(), "Should have text deltas");

        // Last event should be RunFinished with Stop
        let last = events.last().unwrap();
        match last {
            AgentEvent::RunFinished { stop_reason } => {
                assert_eq!(*stop_reason, StopReason::Stop);
            }
            _ => panic!("Expected RunFinished, got: {:?}", last),
        }
    }

    // ── Single tool call event sequence ──

    #[tokio::test]
    async fn single_tool_call_events_in_order() {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_1".into(),
                name: "echo".into(),
                arguments: serde_json::json!({"text": "hello"}),
                stop_reason: None,
            },
            MockStep::Text("Done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(EchoTool));

        let events = collect_events(&mut agent, "echo hello").await;

        assert!(matches!(&events[0], AgentEvent::RunStarted));

        let tool_started = events.iter().find(|e| matches!(e, AgentEvent::ToolStarted { .. }));
        assert!(tool_started.is_some(), "Should have ToolStarted");
        match tool_started.unwrap() {
            AgentEvent::ToolStarted { id, name, .. } => {
                assert_eq!(id, "tc_1");
                assert_eq!(name, "echo");
            }
            _ => unreachable!(),
        }

        let tool_finished = events.iter().find(|e| matches!(e, AgentEvent::ToolFinished { .. }));
        assert!(tool_finished.is_some(), "Should have ToolFinished");
        match tool_finished.unwrap() {
            AgentEvent::ToolFinished { id, result } => {
                assert_eq!(id, "tc_1");
                assert!(!result.is_error);
            }
            _ => unreachable!(),
        }

        let last = events.last().unwrap();
        assert!(matches!(last, AgentEvent::RunFinished { .. }));
    }

    // ── Multiple tool calls event sequence ──

    #[tokio::test]
    async fn multiple_tool_calls_events_in_order() {
        let multi = MultiToolCallProvider::new(
            vec![
                MockStep::ToolCall {
                    id: "tc_a".into(),
                    name: "echo".into(),
                    arguments: serde_json::json!({"text": "alpha"}),
                    stop_reason: None,
                },
                MockStep::ToolCall {
                    id: "tc_b".into(),
                    name: "echo".into(),
                    arguments: serde_json::json!({"text": "beta"}),
                    stop_reason: None,
                },
            ],
            "Both done",
        );
        let mut agent = Agent::new(Box::new(multi), make_model());
        agent.add_tool(Box::new(EchoTool));

        let events = collect_events(&mut agent, "two echoes").await;

        let tool_started: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolStarted { .. }))
            .collect();
        assert_eq!(tool_started.len(), 2, "Should have 2 ToolStarted events");

        let tool_finished: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolFinished { .. }))
            .collect();
        assert_eq!(tool_finished.len(), 2, "Should have 2 ToolFinished events");

        let ids: Vec<&str> = tool_started
            .iter()
            .map(|e| match e {
                AgentEvent::ToolStarted { id, .. } => id.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert!(ids.contains(&"tc_a"));
        assert!(ids.contains(&"tc_b"));
    }

    // ── Provider error event sequence ──

    #[tokio::test]
    async fn provider_error_events_in_order() {
        let mock = MockProvider::new(vec![MockStep::Error("API error".into())]);
        let mut agent = Agent::new(Box::new(mock), make_model());

        let events = collect_events(&mut agent, "trigger error").await;

        assert!(matches!(&events[0], AgentEvent::RunStarted));

        let provider_error = events.iter().find(|e| matches!(e, AgentEvent::ProviderError { .. }));
        assert!(provider_error.is_some(), "Should have ProviderError event");

        let last = events.last().unwrap();
        match last {
            AgentEvent::RunFinished { stop_reason } => {
                assert_eq!(*stop_reason, StopReason::Error);
            }
            _ => panic!("Expected RunFinished with Error, got: {:?}", last),
        }
    }

    // ── Tool error event sequence ──

    #[tokio::test]
    async fn tool_error_events_in_order() {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_fail".into(),
                name: "fail".into(),
                arguments: serde_json::json!({}),
                stop_reason: None,
            },
            MockStep::Text("handled".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(FailTool));

        let events = collect_events(&mut agent, "trigger tool error").await;

        let tool_finished = events.iter().find(|e| matches!(e, AgentEvent::ToolFinished { .. }));
        assert!(tool_finished.is_some());
        match tool_finished.unwrap() {
            AgentEvent::ToolFinished { id, result } => {
                assert_eq!(id, "tc_fail");
                assert!(result.is_error);
            }
            _ => unreachable!(),
        }
    }

    // ── Cancel event sequence ──

    #[tokio::test]
    async fn cancel_events_in_order() {
        // Pre-cancel the token before running, so the agent immediately sees cancellation
        let mock = MockProvider::new(vec![MockStep::Text("never".into())]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        let token = agent.abort_flag();

        // Cancel before running
        token.cancel();

        let events = collect_events(&mut agent, "cancel me").await;

        assert!(matches!(&events[0], AgentEvent::RunStarted));
        let last = events.last().unwrap();
        assert!(
            matches!(
                last,
                AgentEvent::RunAborted
                    | AgentEvent::RunFinished {
                        stop_reason: StopReason::Aborted
                    }
            ),
            "Expected RunAborted or RunFinished(Aborted), got: {:?}",
            last
        );
    }

    // ── Timeout event sequence (tool-level) ──

    #[tokio::test]
    async fn timeout_events_in_order() {
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let bash_tool = crate::coding_agent::tools::bash::BashTool::new(shared_cwd);

        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_timeout".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "sleep 30", "timeout": 1}),
                stop_reason: None,
            },
            MockStep::Text("handled".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(bash_tool));

        let events = collect_events(&mut agent, "timeout test").await;

        let tool_finished = events.iter().find(|e| matches!(e, AgentEvent::ToolFinished { .. }));
        assert!(tool_finished.is_some());
        match tool_finished.unwrap() {
            AgentEvent::ToolFinished { id, result } => {
                assert_eq!(id, "tc_timeout");
                assert!(result.is_error);
            }
            _ => unreachable!(),
        }
    }
}
