//! rusty-pi — Rust rewrite of the pi coding agent.
//!
//! Binary entry point. Supports:
//! - `rusty-pi` — interactive REPL with mock provider
//! - `rusty-pi -p deepseek "prompt"` — use DeepSeek provider
//! - `rusty-pi -p codex "prompt"` — use OpenAI Codex provider

use clap::Parser;
use rusty_pi::agent::session::SessionStorage;
use rusty_pi::agent::session::jsonl::{JsonlSessionCreateOptions, JsonlSessionStorage};
use rusty_pi::agent::session::session::{Session, SessionContextBuildOptions};
use rusty_pi::agent::session::types::iso_timestamp;
use rusty_pi::ai::mock::MockProvider;
use rusty_pi::ai::providers::deepseek::{DEEPSEEK_MODELS, DeepSeekProvider};
use rusty_pi::ai::providers::openai_codex::{OPENAI_CODEX_MODELS, OpenAICodexProvider};
use rusty_pi::ai::providers::{Model, ProviderApi};
use rusty_pi::coding_agent::prompt_session::PromptSession;
use rusty_pi::coding_agent::repl::{self, RunConfig};
use rusty_pi::coding_agent::system_prompt::ContextFile;
use rusty_pi::coding_agent::tools::bash::BashTool;
use rusty_pi::coding_agent::tools::edit::EditTool;
use rusty_pi::coding_agent::tools::read::ReadTool;
use rusty_pi::coding_agent::tools::write::WriteTool;
use rusty_pi::format::OutputFormatter;
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

    /// Launch the Ratatui TUI frontend instead of the print-based REPL
    #[arg(long = "tui")]
    tui: bool,
}

fn build_provider(name: &str, model_id: Option<&str>) -> anyhow::Result<(Box<dyn ProviderApi>, Model)> {
    match name {
        "mock" => Ok((
            Box::new(MockProvider::text("Hello from rusty-pi! I'm a mock LLM provider.")),
            Model {
                id: "mock",
                api: "mock",
            },
        )),
        "deepseek" => {
            let provider = DeepSeekProvider::from_env().ok_or_else(|| {
                let fmt = OutputFormatter::new();
                anyhow::anyhow!(
                    "{}",
                    fmt.error("DEEPSEEK_API_KEY not set. Get your key at https://platform.deepseek.com/api-keys")
                )
            })?;

            let model_id = model_id.unwrap_or("deepseek-v4-flash");
            let model = DEEPSEEK_MODELS.iter().find(|m| m.id == model_id).ok_or_else(|| {
                let fmt = OutputFormatter::new();
                let available: Vec<&str> = DEEPSEEK_MODELS.iter().map(|m| m.id).collect();
                anyhow::anyhow!(
                    "{}",
                    fmt.error(&format!(
                        "Unknown DeepSeek model '{}'. Available: {:?}",
                        model_id, available
                    ))
                )
            })?;

            Ok((Box::new(provider), model.clone()))
        }
        "codex" => {
            let provider = OpenAICodexProvider::from_env().ok_or_else(|| {
                let fmt = OutputFormatter::new();
                anyhow::anyhow!(
                    "{}",
                    fmt.error("OPENAI_CODEX_TOKEN not set. Get your ChatGPT access token from browser devtools.")
                )
            })?;

            let model_id = model_id.unwrap_or("gpt-5.6-sol");
            let model = OPENAI_CODEX_MODELS.iter().find(|m| m.id == model_id).ok_or_else(|| {
                let fmt = OutputFormatter::new();
                let available: Vec<&str> = OPENAI_CODEX_MODELS.iter().map(|m| m.id).collect();
                anyhow::anyhow!(
                    "{}",
                    fmt.error(&format!(
                        "Unknown Codex model '{}'. Available: {:?}",
                        model_id, available
                    ))
                )
            })?;

            Ok((Box::new(provider), model.clone()))
        }
        other => {
            let fmt = OutputFormatter::new();
            anyhow::bail!(
                "{}",
                fmt.error(&format!(
                    "Unknown provider '{}'. Supported: mock, deepseek, codex",
                    other
                ))
            )
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = RustyPiConfig::load();

    // Resolve provider: CLI > config > "mock"
    let provider_name = cli
        .provider
        .as_deref()
        .or(config.default_provider.as_deref())
        .unwrap_or("mock");

    // Resolve model: CLI > config > None (let provider decide)
    let model_id = cli.model.as_deref().or(config.default_model.as_deref());

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
    let session_obj = create_or_resume_session(&agent_dir, &cwd, cli.resume.as_deref()).await?;

    let shared_cwd = Arc::new(RwLock::new(cwd.clone()));

    let bash_tool = BashTool::new(shared_cwd.clone());
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
    let context_files: Vec<ContextFile> = cli
        .context
        .iter()
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
                    let fmt = OutputFormatter::new();
                    eprintln!(
                        "{}",
                        fmt.error(&format!("Context file '{}' is > 1MB, skipping", p.display()))
                    );
                    return None;
                }
            }
            match std::fs::read_to_string(&resolved) {
                Ok(content) => Some(ContextFile {
                    path: resolved,
                    content,
                }),
                Err(e) => {
                    let fmt = OutputFormatter::new();
                    eprintln!(
                        "{}",
                        fmt.error(&format!("Cannot read context file '{}': {}", p.display(), e))
                    );
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

    // Launch TUI or REPL
    if cli.tui {
        run_tui(session).await
    } else {
        let config = RunConfig {
            prompt: cli.prompt,
            session,
        };
        repl::run(config).await
    }
}

/// Launch the Ratatui TUI frontend.
async fn run_tui(session: PromptSession) -> anyhow::Result<()> {
    // TerminalGuard owns all terminal state transitions. Its Drop path also
    // restores the terminal if the event loop returns an error or panics.
    let mut guard =
        rusty_pi::tui::terminal_guard::TerminalGuard::new(rusty_pi::tui::terminal_guard::CrosstermTerminal::new())?;
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;

    let size = terminal.size()?;
    let mut app_state = rusty_pi::tui::app::AppState::new((size.width, size.height));

    // Create agent and event channel
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);
    let mut session = session;
    session.agent().set_event_sender(event_tx);
    let mut current_token = tokio_util::sync::CancellationToken::new();
    session.agent().set_abort_flag(current_token.clone());
    let registry = rusty_pi::coding_agent::repl::default_registry();

    // Run TUI loop — agent runs inline when a prompt is submitted
    let result = run_tui_loop(
        &mut terminal,
        &mut app_state,
        &mut session,
        &registry,
        &mut current_token,
        &mut event_rx,
    )
    .await;

    drop(terminal);
    let restore_result = guard.restore();
    match (result, restore_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Ok(()), Ok(())) => Ok(()),
    }
}

/// TUI event loop.
/// When a prompt is submitted, the agent runs inline (not in a separate task)
/// so we avoid Send/Sync issues with the Agent's callback fields.
async fn run_tui_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    state: &mut rusty_pi::tui::app::AppState,
    session: &mut PromptSession,
    registry: &rusty_pi::coding_agent::command::CommandRegistry,
    current_token: &mut tokio_util::sync::CancellationToken,
    event_rx: &mut tokio::sync::mpsc::Receiver<rusty_pi::agent::events::AgentEvent>,
) -> anyhow::Result<()> {
    use rusty_pi::tui::app::{Action, Effect};

    loop {
        // Draw
        terminal.draw(|frame| {
            rusty_pi::tui::app::view(frame, state);
        })?;

        // Handle events with a timeout so we can process agent events
        let poll_timeout = std::time::Duration::from_millis(50);
        if crossterm::event::poll(poll_timeout)? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                let effects = state.update(Action::KeyInput(key));
                for effect in effects {
                    match effect {
                        Effect::RunAgent(prompt) => {
                            match registry.dispatch(&prompt, session)? {
                                rusty_pi::coding_agent::command::DispatchOutcome::Exit => {
                                    let _ = state.update(Action::Quit);
                                }
                                rusty_pi::coding_agent::command::DispatchOutcome::Handled(result) => {
                                    let command_effects = state.update(Action::CommandResult(result));
                                    if command_effects.iter().any(|effect| matches!(effect, Effect::Quit)) {
                                        return Ok(());
                                    }
                                }
                                rusty_pi::coding_agent::command::DispatchOutcome::NotACommand => {
                                    // Create fresh token for this run
                                    let new_token = tokio_util::sync::CancellationToken::new();
                                    session.agent().set_abort_flag(new_token.clone());
                                    *current_token = new_token;
                                    // Run agent inline — events flow through the channel
                                    let _ = session.agent().run(&prompt).await;
                                }
                            }
                        }
                        Effect::CancelAgent => {
                            current_token.cancel();
                        }
                        Effect::Quit => {
                            return Ok(());
                        }
                    }
                }
            }
        }

        // Process agent events
        while let Ok(event) = event_rx.try_recv() {
            state.update(Action::AgentEvent(event));
        }

        // Check if we should quit
        if state.quit {
            return Ok(());
        }
    }
}

/// Build a formatted session listing string. Returns "No sessions found." or
/// "Available sessions:\n  {id} | created: {created_at} | path: {path}\n...".
async fn format_session_list(sessions_dir: &PathBuf) -> anyhow::Result<String> {
    if !sessions_dir.exists() {
        return Ok(format!("No sessions directory found at: {}", sessions_dir.display()));
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
                sessions.push((
                    meta.id.clone(),
                    meta.created_at.clone(),
                    path.to_string_lossy().to_string(),
                ));
            }
            Err(_) => continue,
        }
    }

    // Sort by created_at descending (newest first)
    sessions.sort_by(|a, b| b.1.cmp(&a.1));

    if sessions.is_empty() {
        Ok("No sessions found.".into())
    } else {
        let mut out = "Available sessions:".to_string();
        for (id, created_at, path) in &sessions {
            use std::fmt::Write;
            write!(&mut out, "\n  {} | created: {} | path: {}", id, created_at, path).unwrap();
        }
        Ok(out)
    }
}

/// List available sessions and exit (thin wrapper that prints).
async fn list_sessions(sessions_dir: &PathBuf) -> anyhow::Result<()> {
    let formatted = format_session_list(sessions_dir).await?;
    println!("{}", formatted);
    Ok(())
}

/// Create a new JSONL session or resume an existing one.
async fn create_or_resume_session(agent_dir: &PathBuf, cwd: &PathBuf, resume: Option<&str>) -> anyhow::Result<Session> {
    match resume {
        Some(path_or_prefix) => {
            let path = std::path::Path::new(path_or_prefix);
            let file_path = if path.exists() {
                path.to_path_buf()
            } else {
                // Try to find by prefix in sessions directory
                let sessions_dir = agent_dir.join("sessions");
                if !sessions_dir.exists() {
                    let fmt = OutputFormatter::new();
                    anyhow::bail!(
                        "{}",
                        fmt.error(&format!(
                            "No session found matching '{}'. Sessions directory does not exist.",
                            path_or_prefix
                        ))
                    );
                }
                let mut matches: Vec<PathBuf> = Vec::new();
                let mut read_dir = tokio::fs::read_dir(&sessions_dir).await?;
                while let Some(entry) = read_dir.next_entry().await? {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("jsonl")
                        && p.file_stem()
                            .and_then(|s| s.to_str())
                            .map_or(false, |s| s.contains(path_or_prefix))
                    {
                        matches.push(p);
                    }
                }
                match matches.len() {
                    0 => {
                        let fmt = OutputFormatter::new();
                        anyhow::bail!(
                            "{}",
                            fmt.error(&format!(
                                "No session found matching '{}' in {}",
                                path_or_prefix,
                                sessions_dir.display()
                            ))
                        );
                    }
                    1 => matches.into_iter().next().unwrap(),
                    _ => {
                        let paths: Vec<String> = matches.iter().map(|p| p.to_string_lossy().to_string()).collect();
                        let fmt = OutputFormatter::new();
                        anyhow::bail!(
                            "{}",
                            fmt.error(&format!(
                                "Multiple sessions match '{}': {}. Use a more specific prefix.",
                                path_or_prefix,
                                paths.join(", ")
                            ))
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
                iso_timestamp().replace(|c: char| !c.is_alphanumeric() && c != '-', "")
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
            )
            .await?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ── Config deserialization (Ticket 16) ─────────────────────────────────

    #[test]
    fn config_parses_valid_toml() {
        let toml_str = "\n            default_provider = \"deepseek\"\n            default_model = \"deepseek-v4-flash\"\n        ";
        let config: RustyPiConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_provider.as_deref(), Some("deepseek"));
        assert_eq!(config.default_model.as_deref(), Some("deepseek-v4-flash"));
    }

    #[test]
    fn config_empty_toml_returns_defaults() {
        let config: RustyPiConfig = toml::from_str("").unwrap();
        assert!(config.default_provider.is_none());
        assert!(config.default_model.is_none());
        assert!(config.prompt_paths.is_none());
        assert!(config.skill_paths.is_none());
    }

    #[test]
    fn config_partial_toml() {
        let toml_str = "\n            prompt_paths = [\"/my/templates\"]\n        ";
        let config: RustyPiConfig = toml::from_str(toml_str).unwrap();
        assert!(config.default_provider.is_none());
        assert!(config.prompt_paths.is_some());
        let paths = config.prompt_paths.unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/my/templates");
    }

    // ── Session listing (Ticket 14) ───────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_empty_dir() {
        let dir = tempdir().unwrap();
        let result = format_session_list(&dir.path().to_path_buf()).await.unwrap();
        assert_eq!(result, "No sessions found.");
    }

    #[tokio::test]
    async fn list_sessions_non_existent_dir() {
        let path = PathBuf::from("/tmp/__rusty_pi_test_nonexistent__");
        let result = format_session_list(&path).await.unwrap();
        assert!(result.starts_with("No sessions directory found at"));
    }

    #[tokio::test]
    async fn list_sessions_with_one_session() {
        let dir = tempdir().unwrap();
        let sessions_dir = dir.path().join("sessions");
        tokio::fs::create_dir_all(&sessions_dir).await.unwrap();

        // Create a real JSONL session file
        let file_path = sessions_dir.join("test-session.jsonl");
        let _storage = JsonlSessionStorage::create(
            file_path.to_string_lossy().to_string(),
            JsonlSessionCreateOptions {
                session_id: "test-session-1".into(),
                cwd: dir.path().to_string_lossy().to_string(),
                parent_session_path: None,
                metadata: None,
            },
        )
        .await
        .unwrap();

        let result = format_session_list(&sessions_dir).await.unwrap();
        assert!(result.starts_with("Available sessions:"));
        assert!(result.contains("test-session-1"));
        assert!(result.contains("| created:"));
        assert!(result.contains("| path:"));
    }

    #[tokio::test]
    async fn list_sessions_ignores_non_jsonl_files() {
        let dir = tempdir().unwrap();
        let sessions_dir = dir.path().join("sessions");
        tokio::fs::create_dir_all(&sessions_dir).await.unwrap();

        // Create a .txt file that should be ignored
        tokio::fs::write(sessions_dir.join("notes.txt"), "hello").await.unwrap();

        let result = format_session_list(&sessions_dir).await.unwrap();
        assert_eq!(result, "No sessions found.");
    }
}
