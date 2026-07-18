//! REPL — Read-Eval-Print Loop for interactive chat.

use crate::agent::engine::{Agent, AbortFlag};
use crate::agent::types::AgentTool;
use crate::ai::providers::{Model, ProviderApi};
use crate::ai::types::{AgentMessage, StopReason};
use anyhow::Result;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Run configuration for the CLI.
pub struct RunConfig {
    /// Optional single-shot prompt. If `None`, enters REPL mode.
    pub prompt: Option<String>,
    /// System prompt to set for the agent.
    pub system_prompt: String,
    /// LLM provider to use.
    pub provider: Box<dyn ProviderApi>,
    /// Model to use with the provider.
    pub model: Model,
    /// Tools to register with the agent.
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

/// Helper: run an agent with Ctrl+C abort support.
/// Returns `true` if the run was aborted, `false` otherwise.
async fn run_with_abort(agent: &mut Agent, prompt: &str) -> bool {
    let abort_flag: AbortFlag = Arc::new(AtomicBool::new(false));
    agent.set_abort_flag(abort_flag.clone());

    let buf = Arc::new(Mutex::new(String::new()));
    let buf_cb = buf.clone();
    agent.on_text(move |delta| {
        print!("{}", delta);
        let _ = io::stdout().flush();
        buf_cb.lock().unwrap().push_str(delta);
    });

    let run_future = agent.run(prompt);
    tokio::pin!(run_future);

    let was_aborted = tokio::select! {
        result = &mut run_future => {
            if let Err(e) = result {
                eprintln!("\n[error] {}", e);
            }
            false
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\n[interrupt: aborting...]");
            abort_flag.store(true, Ordering::SeqCst);
            // tokio::select! drops run_future automatically when this branch wins,
            // which cancels the in-flight agent loop and closes the stream receiver.
            abort_flag.store(false, Ordering::SeqCst);
            true
        }
    };

    // Print trailing newline if needed
    {
        let output = buf.lock().unwrap();
        if !output.ends_with('\n') && !output.is_empty() {
            println!();
        }
    }

    was_aborted
}

/// Run a single prompt and print the response, then exit.
async fn run_single_shot(agent: &mut Agent, prompt: &str) -> Result<()> {
    run_with_abort(agent, prompt).await;
    Ok(())
}

/// Enter an interactive REPL loop.
///
/// Displays a `> ` prompt. Each line is sent to the agent as a user message.
/// The agent streams the response token-by-token. Type `/exit` or `/quit` to
/// leave the loop. Ctrl+C at the prompt exits the REPL; Ctrl+C during agent
/// execution aborts the current round and returns to the prompt.
async fn run_repl(agent: &mut Agent) -> Result<()> {
    println!("rusty-pi REPL (type '/exit' to quit)\n");

    let mut stdout = io::stdout();

    loop {
        // Read a line from stdin asynchronously (spawn_blocking to avoid blocking the reactor)
        let stdin = io::stdin();
        let line_future = tokio::task::spawn_blocking(move || {
            let mut line = String::new();
            stdin.lock().read_line(&mut line).ok()?;
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() { None } else { Some(trimmed) }
        });

        print!("> ");
        stdout.flush()?;

        let line = tokio::select! {
            result = line_future => {
                match result.unwrap_or(None) {
                    Some(line) => line,
                    None => {
                        println!();
                        break;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("^C");
                break;
            }
        };

        if line == "/exit" || line == "/quit" {
            break;
        }

        let aborted = run_with_abort(agent, &line).await;

        if !aborted {
            let msgs = agent.messages();
            if let Some(AgentMessage::Assistant(a)) = msgs.last()
                && a.stop_reason == StopReason::Error
                    && let Some(err) = &a.error_message {
                        eprintln!("[error] {}", err);
                    }
        }
        println!();
    }

    Ok(())
}
