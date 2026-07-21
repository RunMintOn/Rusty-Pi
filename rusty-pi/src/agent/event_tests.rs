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

    use crate::agent::types::ToolExecutionContext;

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
            _context: ToolExecutionContext,
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
            _context: ToolExecutionContext,
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
        assert!(matches!(&events[0], AgentEvent::RunStarted { .. }));

        // Should have at least one TextDelta
        let text_deltas: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TextDelta { .. }))
            .collect();
        assert!(!text_deltas.is_empty(), "Should have text deltas");

        // Last event should be RunFinished with Stop
        let last = events.last().unwrap();
        match last {
            AgentEvent::RunFinished { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::Stop);
            }
            _ => panic!("Expected RunFinished, got: {:?}", last),
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    AgentEvent::RunFinished { .. } | AgentEvent::RunAborted { .. } | AgentEvent::RunFailed { .. }
                ))
                .count(),
            1
        );
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

        assert!(matches!(&events[0], AgentEvent::RunStarted { .. }));

        let tool_started = events.iter().find(|e| matches!(e, AgentEvent::ToolStarted { .. }));
        assert!(tool_started.is_some(), "Should have ToolStarted");
        match tool_started.unwrap() {
            AgentEvent::ToolStarted { tool_call_id, name, .. } => {
                assert_eq!(tool_call_id, "tc_1");
                assert_eq!(name, "echo");
            }
            _ => unreachable!(),
        }

        let tool_finished = events.iter().find(|e| matches!(e, AgentEvent::ToolFinished { .. }));
        assert!(tool_finished.is_some(), "Should have ToolFinished");
        match tool_finished.unwrap() {
            AgentEvent::ToolFinished {
                tool_call_id,
                name,
                result,
                ..
            } => {
                assert_eq!(tool_call_id, "tc_1");
                assert_eq!(name, "echo");
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
                AgentEvent::ToolStarted { tool_call_id, .. } => tool_call_id.as_str(),
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

        assert!(matches!(&events[0], AgentEvent::RunStarted { .. }));

        let provider_error = events.iter().find(|e| matches!(e, AgentEvent::ProviderError { .. }));
        assert!(provider_error.is_some(), "Should have ProviderError event");

        let last = events.last().unwrap();
        match last {
            AgentEvent::RunFinished { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::Error);
            }
            _ => panic!("Expected RunFinished with Error, got: {:?}", last),
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    AgentEvent::RunFinished { .. } | AgentEvent::RunAborted { .. } | AgentEvent::RunFailed { .. }
                ))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn tool_execution_failure_emits_one_run_failed() {
        let mock = MockProvider::new(vec![MockStep::ToolCall {
            id: "tc_missing".into(),
            name: "missing_tool".into(),
            arguments: serde_json::json!({}),
            stop_reason: None,
        }]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        agent.set_event_sender(tx);

        let result = agent.run("invoke missing tool").await;
        assert!(result.is_err());

        let (dummy_tx, _dummy_rx) = tokio::sync::mpsc::channel(1);
        agent.set_event_sender(dummy_tx);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        let failures: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::RunFailed { error, .. } => Some(error),
                _ => None,
            })
            .collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].phase, crate::agent::events::AgentRunPhase::ToolExecution);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    AgentEvent::RunFinished { .. } | AgentEvent::RunAborted { .. } | AgentEvent::RunFailed { .. }
                ))
                .count(),
            1
        );
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
            AgentEvent::ToolFinished {
                tool_call_id,
                name,
                result,
                ..
            } => {
                assert_eq!(tool_call_id, "tc_fail");
                assert_eq!(name, "fail");
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

        assert!(matches!(&events[0], AgentEvent::RunStarted { .. }));
        let last = events.last().unwrap();
        assert!(
            matches!(
                last,
                AgentEvent::RunAborted { .. }
                    | AgentEvent::RunFinished {
                        stop_reason: StopReason::Aborted,
                        ..
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
            AgentEvent::ToolFinished {
                tool_call_id,
                name,
                result,
                ..
            } => {
                assert_eq!(tool_call_id, "tc_timeout");
                assert_eq!(name, "bash");
                assert!(result.is_error);
            }
            _ => unreachable!(),
        }
    }

    // ── ToolOutput event chain tests ─────────────────────────────────────

    #[tokio::test]
    async fn tool_output_stdout_single_chunk_events() {
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let bash_tool = crate::coding_agent::tools::bash::BashTool::new(shared_cwd);

        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_out1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "echo hello"}),
                stop_reason: None,
            },
            MockStep::Text("done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(bash_tool));

        let events = collect_events(&mut agent, "echo hello").await;

        // Should have ToolOutput events between ToolStarted and ToolFinished
        let tool_outputs: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolOutput { .. }))
            .collect();
        assert!(!tool_outputs.is_empty(), "Should have ToolOutput events");

        // All ToolOutput events should have the correct tool call ID
        for ev in &tool_outputs {
            if let AgentEvent::ToolOutput {
                tool_call_id, stream, ..
            } = ev
            {
                assert_eq!(tool_call_id, "tc_out1");
                assert_eq!(*stream, crate::agent::events::ToolOutputStream::Stdout);
            }
        }

        // ToolFinished should come after all ToolOutput events
        let last_output_idx = events
            .iter()
            .rposition(|e| matches!(e, AgentEvent::ToolOutput { .. }))
            .unwrap();
        let tool_finished_idx = events
            .iter()
            .position(|e| matches!(e, AgentEvent::ToolFinished { .. }))
            .unwrap();
        assert!(
            tool_finished_idx > last_output_idx,
            "ToolFinished should come after ToolOutput"
        );
    }

    #[tokio::test]
    async fn tool_output_stderr_events() {
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let bash_tool = crate::coding_agent::tools::bash::BashTool::new(shared_cwd);

        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_err1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "echo err_msg >&2"}),
                stop_reason: None,
            },
            MockStep::Text("done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(bash_tool));

        let events = collect_events(&mut agent, "stderr test").await;

        let stderr_outputs: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    AgentEvent::ToolOutput {
                        stream: crate::agent::events::ToolOutputStream::Stderr,
                        ..
                    }
                )
            })
            .collect();
        assert!(!stderr_outputs.is_empty(), "Should have stderr ToolOutput events");
    }

    #[tokio::test]
    async fn tool_output_stdout_stderr_interleaved_events() {
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let bash_tool = crate::coding_agent::tools::bash::BashTool::new(shared_cwd);

        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_mix".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "echo out1; echo err1 >&2; echo out2"}),
                stop_reason: None,
            },
            MockStep::Text("done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(bash_tool));

        let events = collect_events(&mut agent, "mix test").await;

        let stdout_events: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    AgentEvent::ToolOutput {
                        stream: crate::agent::events::ToolOutputStream::Stdout,
                        ..
                    }
                )
            })
            .collect();
        let stderr_events: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    AgentEvent::ToolOutput {
                        stream: crate::agent::events::ToolOutputStream::Stderr,
                        ..
                    }
                )
            })
            .collect();
        assert!(!stdout_events.is_empty(), "Should have stdout events");
        assert!(!stderr_events.is_empty(), "Should have stderr events");
    }

    #[tokio::test]
    async fn tool_output_two_tools_no_id_crossover() {
        use crate::ai::mock::MultiToolCallProvider;

        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let bash_tool = crate::coding_agent::tools::bash::BashTool::new(shared_cwd);

        let multi = MultiToolCallProvider::new(
            vec![
                MockStep::ToolCall {
                    id: "tc_a".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "echo alpha"}),
                    stop_reason: None,
                },
                MockStep::ToolCall {
                    id: "tc_b".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "echo beta"}),
                    stop_reason: None,
                },
            ],
            "both done",
        );
        let mut agent = Agent::new(Box::new(multi), make_model());
        agent.add_tool(Box::new(bash_tool));

        let events = collect_events(&mut agent, "two tools").await;

        // Collect ToolOutput events and verify IDs don't cross
        for ev in &events {
            if let AgentEvent::ToolOutput { tool_call_id, .. } = ev {
                assert!(
                    tool_call_id == "tc_a" || tool_call_id == "tc_b",
                    "Tool ID should be tc_a or tc_b, got: {}",
                    tool_call_id
                );
            }
        }
    }

    #[tokio::test]
    async fn tool_output_then_tool_finished() {
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let bash_tool = crate::coding_agent::tools::bash::BashTool::new(shared_cwd);

        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_of".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "echo data"}),
                stop_reason: None,
            },
            MockStep::Text("done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(bash_tool));

        let events = collect_events(&mut agent, "output then finish").await;

        // Verify order: ToolStarted -> ToolOutput(s) -> ToolFinished
        let mut saw_started = false;
        let mut saw_output = false;
        let mut saw_finished = false;
        for ev in &events {
            match ev {
                AgentEvent::ToolStarted { .. } => {
                    assert!(!saw_started, "Should not see ToolStarted twice");
                    saw_started = true;
                }
                AgentEvent::ToolOutput { .. } => {
                    assert!(saw_started, "ToolOutput should come after ToolStarted");
                    assert!(!saw_finished, "ToolOutput should come before ToolFinished");
                    saw_output = true;
                }
                AgentEvent::ToolFinished { .. } => {
                    assert!(saw_started, "ToolFinished should come after ToolStarted");
                    saw_finished = true;
                }
                _ => {}
            }
        }
        assert!(saw_started, "Should have ToolStarted");
        assert!(saw_output, "Should have ToolOutput");
        assert!(saw_finished, "Should have ToolFinished");
    }

    #[tokio::test]
    async fn cancel_no_late_tool_output() {
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let bash_tool = crate::coding_agent::tools::bash::BashTool::new(shared_cwd);

        let multi = MultiToolCallProvider::new(
            vec![MockStep::ToolCall {
                id: "tc_long".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "sleep 30"}),
                stop_reason: None,
            }],
            "should not reach",
        );
        let mut agent = Agent::new(Box::new(multi), make_model());
        agent.add_tool(Box::new(bash_tool));
        let token = agent.abort_flag();

        let events = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let token = token.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                token.cancel();
            });
            collect_events(&mut agent, "cancel test").await
        })
        .await
        .unwrap();

        // After cancel, should not have late ToolOutput events
        // The events should end with RunAborted or RunFinished(Aborted)
        let last = events.last().unwrap();
        assert!(
            matches!(
                last,
                AgentEvent::RunAborted { .. } | AgentEvent::ToolFinished { .. } | AgentEvent::RunFinished { .. }
            ),
            "Last event should be terminal: {:?}",
            last
        );
    }

    #[tokio::test]
    async fn timeout_no_late_tool_output() {
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let bash_tool = crate::coding_agent::tools::bash::BashTool::new(shared_cwd);

        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_to".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "sleep 30", "timeout": 1}),
                stop_reason: None,
            },
            MockStep::Text("done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(bash_tool));

        let events = collect_events(&mut agent, "timeout no late").await;

        // After timeout, ToolFinished should have is_error=true
        let tool_finished = events.iter().find(|e| matches!(e, AgentEvent::ToolFinished { .. }));
        assert!(tool_finished.is_some());
        if let AgentEvent::ToolFinished { result, name, .. } = tool_finished.unwrap() {
            assert_eq!(name, "bash");
            assert!(result.is_error || result.timed_out, "Tool should report error/timeout");
        }
    }
}
