//! rusty-pi — Rust rewrite of the pi coding agent.
//!
//! Binary entry point. Supports:
//! - `rusty-pi` — interactive REPL with mock provider
//! - `rusty-pi -p deepseek "prompt"` — use DeepSeek provider
//! - `rusty-pi -p codex "prompt"` — use OpenAI Codex provider

use clap::Parser;
use rusty_pi::agent::session::jsonl::{JsonlSessionCreateOptions, JsonlSessionStorage};
use rusty_pi::agent::session::session::{Session, SessionContextBuildOptions};
use rusty_pi::agent::session::SessionStorage;
use rusty_pi::ai::mock::MockProvider;
use rusty_pi::ai::providers::deepseek::{DeepSeekProvider, DEEPSEEK_MODELS};
use rusty_pi::ai::providers::openai_codex::{OpenAICodexProvider, OPENAI_CODEX_MODELS};
use rusty_pi::ai::providers::{Model, ProviderApi};
use rusty_pi::coding_agent::prompt_session::PromptSession;
use rusty_pi::coding_agent::system_prompt::ContextFile;
use rusty_pi::agent::session::types::iso_timestamp;
use rusty_pi::coding_agent::repl::{self, RunConfig};
use rusty_pi::coding_agent::tools::bash::BashTool;
use rusty_pi::coding_agent::tools::edit::EditTool;
use rusty_pi::coding_agent::tools::read::ReadTool;
use rusty_pi::coding_agent::tools::write::WriteTool;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Default configuration (TOML file).
#[derive(Debug, Default, serde::Deserialize)]
struct RustyPiConfig {
    default_provider: Option<String>,
    default_model: Option<String>,
    prompt_paths: Option<Vec<String>>,
    skill_paths: Option<Vec<String>>,
}

impl RustyPiConfig {
    /// Load config from the first existing config file.
    fn load() -> Self {
        let paths = config_paths();
        for p in &paths {
            if p.exists() {
                let content = match std::fs::read_to_string(p) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                if let Ok(config) = toml::from_str::<RustyPiConfig>(&content) {
                    return config;
                }
            }
        }
        RustyPiConfig::default()
    }
}

/// Config file paths, in priority order.
fn config_paths() -> Vec<PathBuf> {
    let agent_dir = get_agent_dir();
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    vec![
        agent_dir.join("config.toml"),
        PathBuf::from(&home).join(".rusty-pi.toml"),
        PathBuf::from(".rusty-pi.toml"),
    ]
}

#[derive(Parser)]
#[command(name = "rusty-pi", version, about = "Rust rewrite of earendil-works/pi")]
struct Cli {
    /// Provider to use (mock, deepseek, codex). Default from config or "mock".
    #[arg(short, long)]
    provider: Option<String>,

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

    /// Resume a previous session (path or partial filename match)
    #[arg(short = 'r', long = "resume")]
    resume: Option<String>,

    /// List available sessions and exit
    #[arg(long = "list-sessions")]
    list_sessions: bool,

    /// Path to context file(s) whose content is injected into the system prompt
    #[arg(short = 'c', long = "context")]
    context: Vec<PathBuf>,
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
    let config = RustyPiConfig::load();

    // Resolve provider: CLI > config > "mock"
    let provider_name = cli.provider.as_deref()
        .or(config.default_provider.as_deref())
        .unwrap_or("mock");

    // Resolve model: CLI > config > None (let provider decide)
    let model_id = cli.model.as_deref()
        .or(config.default_model.as_deref());

    let (provider, model) = build_provider(provider_name, model_id)?;
    let cwd = std::env::current_dir()?;
    let agent_dir = get_agent_dir();

    // Handle --list-sessions
    if cli.list_sessions {
        let sessions_dir = agent_dir.join("sessions");
        list_sessions(&sessions_dir).await?;
        return Ok(());
    }

    // Create or resume JSONL session
    let session_obj = create_or_resume_session(
        &agent_dir,
        &cwd,
        cli.resume.as_deref(),
    ).await?;

    let shared_cwd = Arc::new(RwLock::new(cwd.clone()));

    let mut bash_tool = BashTool::new(shared_cwd.clone());
    bash_tool.on_output(|chunk| {
        use std::io::Write;
        let _ = write!(std::io::stdout(), "{}", chunk);
        let _ = std::io::stdout().flush();
    });
    let read_tool = ReadTool::new(shared_cwd.clone());
    let write_tool = WriteTool::new(shared_cwd.clone());
    let edit_tool = EditTool::new(shared_cwd.clone());

    let tools: Vec<Box<dyn rusty_pi::agent::types::AgentTool>> = vec![
        Box::new(bash_tool),
        Box::new(read_tool),
        Box::new(write_tool),
        Box::new(edit_tool),
    ];

    // Merge config prompt_paths/skill_paths with CLI args
    let mut prompt_paths = cli.prompt_paths;
    if let Some(ref paths) = config.prompt_paths {
        for p in paths {
            let resolved = if p.starts_with("~") {
                let home = std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_else(|_| ".".into());
                PathBuf::from(home).join(&p[2..])
            } else {
                PathBuf::from(p)
            };
            if !prompt_paths.contains(&resolved) {
                prompt_paths.push(resolved);
            }
        }
    }
    let mut skill_paths = cli.skill_paths;
    if let Some(ref paths) = config.skill_paths {
        for p in paths {
            let resolved = if p.starts_with("~") {
                let home = std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_else(|_| ".".into());
                PathBuf::from(home).join(&p[2..])
            } else {
                PathBuf::from(p)
            };
            if !skill_paths.contains(&resolved) {
                skill_paths.push(resolved);
            }
        }
    }

    // Load context files
    let context_files: Vec<ContextFile> = cli.context.iter()
        .filter_map(|p| {
            let resolved = if p.is_relative() {
                let cwd = std::env::current_dir().ok()?;
                cwd.join(p)
            } else {
                p.clone()
            };
            // Size check (> 1MB → warn and skip)
            if let Ok(meta) = std::fs::metadata(&resolved) {
                if meta.len() > 1_048_576 {
                    eprintln!("[warning] Context file '{}' is > 1MB, skipping", p.display());
                    return None;
                }
            }
            match std::fs::read_to_string(&resolved) {
                Ok(content) => {
                    Some(ContextFile {
                        path: resolved,
                        content,
                    })
                }
                Err(e) => {
                    eprintln!("[warning] Cannot read context file '{}': {}", p.display(), e);
                    None
                }
            }
        })
        .collect();

    let session = PromptSession::new(
        provider,
        model,
        tools,
        cwd,
        agent_dir,
        prompt_paths,
        skill_paths,
        true, // include_defaults
        Some(session_obj),
        context_files,
    );

    let config = RunConfig {
        prompt: cli.prompt,
        session,
    };

    repl::run(config).await
}

/// List available sessions and exit.
async fn list_sessions(sessions_dir: &PathBuf) -> anyhow::Result<()> {
    if !sessions_dir.exists() {
        println!("No sessions directory found at: {}", sessions_dir.display());
        return Ok(());
    }

    let mut read_dir = tokio::fs::read_dir(sessions_dir).await?;
    let mut sessions: Vec<(String, String, String)> = Vec::new(); // (id, created_at, path)

    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        // Try to parse header
        match JsonlSessionStorage::open(path.to_string_lossy().to_string()).await {
            Ok(storage) => {
                let meta = storage.get_metadata().await;
                sessions.push((meta.id.clone(), meta.created_at.clone(), path.to_string_lossy().to_string()));
            }
            Err(_) => continue,
        }
    }

    // Sort by created_at descending (newest first)
    sessions.sort_by(|a, b| b.1.cmp(&a.1));

    if sessions.is_empty() {
        println!("No sessions found.");
    } else {
        println!("Available sessions:");
        for (id, created_at, path) in &sessions {
            println!("  {} | created: {} | path: {}", id, created_at, path);
        }
    }

    Ok(())
}

/// Create a new JSONL session or resume an existing one.
async fn create_or_resume_session(
    agent_dir: &PathBuf,
    cwd: &PathBuf,
    resume: Option<&str>,
) -> anyhow::Result<Session> {
    match resume {
        Some(path_or_prefix) => {
            let path = std::path::Path::new(path_or_prefix);
            let file_path = if path.exists() {
                path.to_path_buf()
            } else {
                // Try to find by prefix in sessions directory
                let sessions_dir = agent_dir.join("sessions");
                if !sessions_dir.exists() {
                    anyhow::bail!(
                        "No session found matching '{}'. Sessions directory does not exist.",
                        path_or_prefix
                    );
                }
                let mut matches: Vec<PathBuf> = Vec::new();
                let mut read_dir = tokio::fs::read_dir(&sessions_dir).await?;
                while let Some(entry) = read_dir.next_entry().await? {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("jsonl")
                        && p.file_stem().and_then(|s| s.to_str()).map_or(false, |s| s.contains(path_or_prefix))
                    {
                        matches.push(p);
                    }
                }
                match matches.len() {
                    0 => anyhow::bail!(
                        "No session found matching '{}' in {}",
                        path_or_prefix,
                        sessions_dir.display()
                    ),
                    1 => matches.into_iter().next().unwrap(),
                    _ => {
                        let paths: Vec<String> = matches.iter().map(|p| p.to_string_lossy().to_string()).collect();
                        anyhow::bail!(
                            "Multiple sessions match '{}': {}. Use a more specific prefix.",
                            path_or_prefix,
                            paths.join(", ")
                        );
                    }
                }
            };

            let storage = JsonlSessionStorage::open(file_path.to_string_lossy().to_string()).await?;
            Ok(Session::new(Box::new(storage), SessionContextBuildOptions::default()))
        }
        None => {
            // Create new session
            let sessions_dir = agent_dir.join("sessions");
            tokio::fs::create_dir_all(&sessions_dir).await?;

            let session_id = format!(
                "session-{}",
                iso_timestamp()
                    .replace(|c: char| !c.is_alphanumeric() && c != '-', "")
            );
            let file_path = sessions_dir.join(format!("{}.jsonl", session_id));

            let storage = JsonlSessionStorage::create(
                file_path.to_string_lossy().to_string(),
                JsonlSessionCreateOptions {
                    session_id: session_id.clone(),
                    cwd: cwd.to_string_lossy().to_string(),
                    parent_session_path: None,
                    metadata: None,
                },
            ).await?;

            Ok(Session::new(Box::new(storage), SessionContextBuildOptions::default()))
        }
    }
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
