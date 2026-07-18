//! REPL — Read-Eval-Print Loop for interactive chat.

use crate::agent::engine::Agent;
use crate::agent::types::AgentTool;
use crate::ai::providers::{Model, ProviderApi};
use crate::ai::types::{AgentMessage, StopReason};
use anyhow::Result;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

/// Run configuration for the CLI.
pub struct RunConfig {
    pub prompt: Option<String>,
    pub system_prompt: String,
    pub provider: Box<dyn ProviderApi>,
    pub model: Model,
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

async fn run_single_shot(agent: &mut Agent, prompt: &str) -> Result<()> {
    let buf = Arc::new(Mutex::new(String::new()));
    let buf_cb = buf.clone();
    agent.on_text(move |delta| {
        print!("{}", delta);
        let _ = io::stdout().flush();
        buf_cb.lock().unwrap().push_str(delta);
    });

    agent.run(prompt).await?;

    let output = buf.lock().unwrap().clone();
    if !output.ends_with('\n') && !output.is_empty() {
        println!();
    }

    Ok(())
}

async fn run_repl(agent: &mut Agent) -> Result<()> {
    println!("rusty-pi REPL (type '/exit' to quit)\n");

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("> ");
        stdout.flush()?;

        let mut line = String::new();
        let bytes_read = stdin.lock().read_line(&mut line)?;
        if bytes_read == 0 {
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

        // Use Arc<Mutex> for the streaming callback
        let text_output = Arc::new(Mutex::new(String::new()));
        let to = text_output.clone();
        agent.on_text(move |delta| {
            print!("{}", delta);
            let _ = io::stdout().flush();
            to.lock().unwrap().push_str(delta);
        });

        match agent.run(&line).await {
            Ok(()) => {
                let msgs = agent.messages();
                if let Some(AgentMessage::Assistant(a)) = msgs.last()
                    && a.stop_reason == StopReason::Error
                        && let Some(err) = &a.error_message {
                            eprintln!("\n[error] {}", err);
                        }
            }
            Err(e) => {
                eprintln!("\n[error] {}", e);
            }
        }

        {
            let output = text_output.lock().unwrap();
            if !output.ends_with('\n') && !output.is_empty() {
                println!();
            }
        }
        println!();
    }

    Ok(())
}
