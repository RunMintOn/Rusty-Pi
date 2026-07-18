//! REPL — Read-Eval-Print Loop for interactive chat.

use crate::agent::engine::AbortFlag;
use crate::ai::types::{AgentMessage, StopReason};
use crate::coding_agent::prompt_session::PromptSession;
use anyhow::Result;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::{self, Write};
use std::path::PathBuf;
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

/// Resolve history path given a home directory string.
fn history_path_for_home(home: &str) -> PathBuf {
    PathBuf::from(home).join(".pi").join("agent").join("repl-history.txt")
}

/// Get the REPL history file path.
fn history_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    history_path_for_home(&home)
}

/// Build the /help message text.
fn help_text() -> String {
    let mut out = String::new();
    out.push_str("\n");
    out.push_str("  Commands:\n");
    out.push_str("    /exit, /quit   Exit the REPL\n");
    out.push_str("    /help          Show this help message\n");
    out.push_str("\n");
    out.push_str("  Tips:\n");
    out.push_str("    - Up/down arrows navigate command history\n");
    out.push_str("    - Ctrl+C at prompt exits\n");
    out.push_str("    - Ctrl+C during agent run aborts the current round\n");
    out.push_str("    - Type any text to chat with the agent\n");
    out.push_str("\n");
    out
}

/// Print the /help message.
fn print_help() {
    print!("{}", help_text());
}

/// Enter an interactive REPL loop.
///
/// Displays a `> ` prompt. Each line is sent to the agent as a user message.
/// The agent streams the response token-by-token. Type `/exit` or `/quit` to
/// leave the loop. Ctrl+C at the prompt exits the REPL; Ctrl+C during agent
/// execution aborts the current round and returns to the prompt.
async fn run_repl(session: &mut PromptSession) -> Result<()> {
    let mut rl = DefaultEditor::new()
        .map_err(|e| anyhow::anyhow!("Failed to create REPL editor: {}", e))?;

    // Load history
    let hist_path = history_path();
    if let Some(parent) = hist_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = rl.load_history(&hist_path);

    println!("rusty-pi REPL (type '/exit' to quit, '/help' for help)\n");

    loop {
        let line = match rl.readline("> ") {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C at prompt → exit
                println!("^C");
                break;
            }
            Err(ReadlineError::Eof) => {
                // Ctrl+D → exit
                println!();
                break;
            }
            Err(e) => {
                eprintln!("[error] Input error: {}", e);
                break;
            }
        };

        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }

        let _ = rl.add_history_entry(&trimmed);

        // Handle built-in commands
        match trimmed.as_str() {
            "/exit" | "/quit" => break,
            "/help" => {
                print_help();
                continue;
            }
            _ => {}
        }

        let aborted = run_with_abort(session, &trimmed).await;

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

    // Save history
    let _ = rl.append_history(&hist_path);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_path_uses_home_env() {
        let path = history_path_for_home("/home/user");
        assert_eq!(
            path,
            PathBuf::from("/home/user/.pi/agent/repl-history.txt")
        );
    }

    #[test]
    fn history_path_handles_trailing_slash() {
        let path = history_path_for_home("/home/user/");
        assert_eq!(
            path,
            PathBuf::from("/home/user/.pi/agent/repl-history.txt")
        );
    }

    #[test]
    fn help_text_contains_commands() {
        let text = help_text();
        assert!(text.contains("/exit"));
        assert!(text.contains("/quit"));
        assert!(text.contains("/help"));
    }

    #[test]
    fn help_text_contains_tips() {
        let text = help_text();
        assert!(text.contains("Up/down arrows"));
        assert!(text.contains("Ctrl+C"));
    }
}
