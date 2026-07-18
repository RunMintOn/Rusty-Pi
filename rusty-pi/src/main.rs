//! rusty-pi — Rust rewrite of the pi coding agent.
//!
//! Binary entry point. Supports:
//! - `rusty-pi` — interactive REPL with mock provider
//! - `rusty-pi -p deepseek "prompt"` — use DeepSeek provider
//! - `rusty-pi -p codex "prompt"` — use OpenAI Codex provider

use clap::Parser;
use rusty_pi::ai::mock::MockProvider;
use rusty_pi::ai::providers::deepseek::{DeepSeekProvider, DEEPSEEK_MODELS};
use rusty_pi::ai::providers::openai_codex::{OpenAICodexProvider, OPENAI_CODEX_MODELS};
use rusty_pi::ai::providers::{Model, ProviderApi};
use rusty_pi::coding_agent::prompt_session::PromptSession;
use rusty_pi::coding_agent::repl::{self, RunConfig};
use rusty_pi::coding_agent::tools::bash::BashTool;
use rusty_pi::coding_agent::tools::edit::EditTool;
use rusty_pi::coding_agent::tools::read::ReadTool;
use rusty_pi::coding_agent::tools::write::WriteTool;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rusty-pi", version, about = "Rust rewrite of earendil-works/pi")]
struct Cli {
    /// Provider to use (mock, deepseek, codex)
    #[arg(short, long, default_value = "mock")]
    provider: String,

    /// Model to use
    #[arg(short, long)]
    model: Option<String>,

    /// Path to prompt templates (file or directory, repeatable)
    #[arg(short = 'P', long = "prompt-path")]
    prompt_paths: Vec<PathBuf>,

    /// Path to skills (file or directory, repeatable)
    #[arg(short = 'S', long = "skill-path")]
    skill_paths: Vec<PathBuf>,

    /// Single prompt to run (omit for interactive REPL)
    prompt: Option<String>,
}

fn build_provider(name: &str, model_id: Option<&str>) -> anyhow::Result<(Box<dyn ProviderApi>, Model)> {
    match name {
        "mock" => Ok((
            Box::new(MockProvider::text(
                "Hello from rusty-pi! I'm a mock LLM provider.",
            )),
            Model { id: "mock", api: "mock" },
        )),
        "deepseek" => {
            let provider = DeepSeekProvider::from_env().ok_or_else(|| {
                anyhow::anyhow!(
                    "DEEPSEEK_API_KEY environment variable not set.\n\
                     Get your API key at https://platform.deepseek.com/api-keys"
                )
            })?;

            let model_id = model_id.unwrap_or("deepseek-v4-flash");
            let model = DEEPSEEK_MODELS
                .iter()
                .find(|m| m.id == model_id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Unknown DeepSeek model '{}'. Available: {:?}",
                        model_id,
                        DEEPSEEK_MODELS.iter().map(|m| m.id).collect::<Vec<_>>()
                    )
                })?;

            Ok((Box::new(provider), model.clone()))
        }
        "codex" => {
            let provider = OpenAICodexProvider::from_env().ok_or_else(|| {
                anyhow::anyhow!(
                    "OPENAI_CODEX_TOKEN environment variable not set.\n\
                     Get your ChatGPT access token from the browser devtools."
                )
            })?;

            let model_id = model_id.unwrap_or("gpt-5.6-sol");
            let model = OPENAI_CODEX_MODELS
                .iter()
                .find(|m| m.id == model_id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Unknown Codex model '{}'. Available: {:?}",
                        model_id,
                        OPENAI_CODEX_MODELS.iter().map(|m| m.id).collect::<Vec<_>>()
                    )
                })?;

            Ok((Box::new(provider), model.clone()))
        }
        other => anyhow::bail!("Unknown provider '{}'. Supported: mock, deepseek, codex", other),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let (provider, model) = build_provider(&cli.provider, cli.model.as_deref())?;
    let cwd = std::env::current_dir()?;
    let agent_dir = get_agent_dir();

    let mut bash_tool = BashTool::new(cwd.to_string_lossy().to_string());
    bash_tool.on_output(|chunk| {
        use std::io::Write;
        let _ = write!(std::io::stdout(), "{}", chunk);
        let _ = std::io::stdout().flush();
    });
    let read_tool = ReadTool::new(cwd.to_string_lossy().to_string());
    let write_tool = WriteTool::new(cwd.to_string_lossy().to_string());
    let edit_tool = EditTool::new(cwd.to_string_lossy().to_string());

    let tools: Vec<Box<dyn rusty_pi::agent::types::AgentTool>> = vec![
        Box::new(bash_tool),
        Box::new(read_tool),
        Box::new(write_tool),
        Box::new(edit_tool),
    ];

    let session = PromptSession::new(
        provider,
        model,
        tools,
        cwd,
        agent_dir,
        cli.prompt_paths,
        cli.skill_paths,
        true, // include_defaults
    );

    let config = RunConfig {
        prompt: cli.prompt,
        session,
    };

    repl::run(config).await
}

/// Get the agent config directory (e.g., `~/.pi/agent/`).
fn get_agent_dir() -> PathBuf {
    // Check environment variable override first
    if let Ok(dir) = std::env::var("RUSTY_PI_AGENT_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".pi").join("agent")
}
