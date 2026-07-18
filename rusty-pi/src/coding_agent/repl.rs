//! REPL — Read-Eval-Print Loop for interactive chat.

use crate::agent::engine::AbortFlag;
use crate::ai::types::{AgentMessage, StopReason};
use crate::coding_agent::prompt_session::PromptSession;
use anyhow::Result;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Run configuration for the CLI.
pub struct RunConfig {
    /// Optional single-shot prompt. If `None`, enters REPL mode.
    pub prompt: Option<String>,
    /// Session that wraps the agent with prompt template and skill expansion.
    pub session: PromptSession,
}

/// Run the CLI with the given configuration.
pub async fn run(config: RunConfig) -> Result<()> {
    let mut session = config.session;

    match config.prompt {
        Some(prompt) => run_single_shot(&mut session, &prompt).await,
        None => run_repl(&mut session).await,
    }
}

/// Helper: run an agent with Ctrl+C abort support.
/// Returns `true` if the run was aborted, `false` otherwise.
async fn run_with_abort(session: &mut PromptSession, prompt: &str) -> bool {
    // Expand templates/skills before borrowing agent
    let expanded = session.expand(prompt);

    let agent = session.agent();
    let abort_flag: AbortFlag = Arc::new(AtomicBool::new(false));
    agent.set_abort_flag(abort_flag.clone());

    let buf = Arc::new(Mutex::new(String::new()));
    let buf_cb = buf.clone();
    agent.on_text(move |delta| {
        print!("{}", delta);
        let _ = io::stdout().flush();
        buf_cb.lock().unwrap().push_str(delta);
    });

    let run_future = agent.run(&expanded);
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
async fn run_single_shot(session: &mut PromptSession, prompt: &str) -> Result<()> {
    run_with_abort(session, prompt).await;
    Ok(())
}

/// Enter an interactive REPL loop.
///
/// Displays a `> ` prompt. Each line is sent to the agent as a user message.
/// The agent streams the response token-by-token. Type `/exit` or `/quit` to
/// leave the loop. Ctrl+C at the prompt exits the REPL; Ctrl+C during agent
/// execution aborts the current round and returns to the prompt.
async fn run_repl(session: &mut PromptSession) -> Result<()> {
    println!("rusty-pi REPL (type '/exit' to quit)\n");

    let mut stdout = io::stdout();

    loop {
        // Read a line from stdin asynchronously
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

        let aborted = run_with_abort(session, &line).await;

        if !aborted {
            let agent = session.agent();
            let msgs = agent.messages().await;
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
