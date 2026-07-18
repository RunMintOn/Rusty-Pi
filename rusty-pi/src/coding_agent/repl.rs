//! REPL — Read-Eval-Print Loop for interactive chat.
//!
//! Mirrors a minimal subset of the original `@earendil-works/pi-coding-agent`
//! CLI interface. Supports interactive REPL mode and single-shot prompt mode.

use crate::agent::engine::Agent;
use crate::agent::types::AgentTool;
use crate::ai::providers::{Model, ProviderApi};
use crate::ai::types::{AgentMessage, AssistantContent, StopReason};
use anyhow::Result;
use std::io::{self, BufRead, Write};

/// Run configuration for the CLI.
pub struct RunConfig {
    /// Prompt for single-shot mode. If None, enters REPL.
    pub prompt: Option<String>,
    /// System prompt.
    pub system_prompt: String,
    /// Provider instance.
    pub provider: Box<dyn ProviderApi>,
    /// Model to use.
    pub model: Model,
    /// Tools to register.
    pub tools: Vec<Box<dyn AgentTool>>,
}

/// Run the CLI with the given configuration.
pub async fn run(config: RunConfig) -> Result<()> {
    let mut agent = Agent::new(config.provider, config.model);
    agent.set_system_prompt(config.system_prompt);

    for tool in config.tools {
        agent.add_tool(tool);
    }

    match config.prompt {
        Some(prompt) => run_single_shot(&mut agent, &prompt).await,
        None => run_repl(&mut agent).await,
    }
}

/// Single-shot mode: run one prompt and exit.
async fn run_single_shot(agent: &mut Agent, prompt: &str) -> Result<()> {
    agent.run(prompt).await?;
    let msgs = agent.messages();
    print_messages(&msgs.into_iter().cloned().collect::<Vec<_>>());
    Ok(())
}

/// REPL mode: interactive chat loop.
async fn run_repl(agent: &mut Agent) -> Result<()> {
    println!("rusty-pi REPL (type '/exit' to quit, '/clear' to reset)\n");

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("> ");
        stdout.flush()?;

        let mut line = String::new();
        let bytes_read = stdin.lock().read_line(&mut line)?;
        if bytes_read == 0 {
            // EOF
            println!();
            break;
        }

        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        if line == "/exit" || line == "/quit" {
            break;
        }

        if line == "/clear" {
            // Reset conversation by recreating the agent
            // For now, this is a no-op since we can't easily reset Agent's messages
            // without recreating it. This will be properly implemented with session support.
            println!("[Conversation reset not yet implemented in MVP]");
            continue;
        }

        // Run the prompt through the agent
        match agent.run(&line).await {
            Ok(()) => {
                // Print the last assistant message
                let msgs = agent.messages();
                if let Some(AgentMessage::Assistant(a)) = msgs.last() {
                    for content in &a.content {
                        match content {
                            AssistantContent::Text { text } => println!("{}", text),
                            AssistantContent::Thinking { thinking } => {
                                println!("[thinking] {}", thinking)
                            }
                            AssistantContent::ToolCall { name, arguments, .. } => {
                                println!("[tool call: {}]\n{}", name, serde_json::to_string_pretty(arguments).unwrap_or_default());
                            }
                        }
                    }
                    if a.stop_reason == StopReason::Error
                        && let Some(err) = &a.error_message
                    {
                        eprintln!("[error] {}", err);
                    }
                }
            }
            Err(e) => {
                eprintln!("[error] {}", e);
            }
        }

        println!();
    }

    Ok(())
}

/// Print the reason for an assistant message's stop.
fn stop_reason_display(reason: &StopReason) -> &'static str {
    match reason {
        StopReason::Stop => "stop",
        StopReason::Length => "length",
        StopReason::ToolUse => "tool_use",
        StopReason::Error => "error",
        StopReason::Aborted => "aborted",
    }
}

/// Print a list of messages to stdout.
fn print_messages(msgs: &[AgentMessage]) {
    for msg in msgs {
        match msg {
            AgentMessage::User(u) => {
                println!("[user] {:?}", u.content);
            }
            AgentMessage::Assistant(a) => {
                println!("[assistant] {}", stop_reason_display(&a.stop_reason));
                for content in &a.content {
                    match content {
                        AssistantContent::Text { text } => println!("  {}", text),
                        AssistantContent::Thinking { thinking } => println!("  [thinking] {}", thinking),
                        AssistantContent::ToolCall { name, arguments, .. } => {
                            println!("  [tool: {}] {}", name, arguments);
                        }
                    }
                }
            }
            AgentMessage::ToolResult(tr) => {
                println!("[tool result: {}] ({} blocks)", tr.tool_name, tr.content.len());
            }
        }
    }
}
