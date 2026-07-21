//! PromptSession — a thin session layer that wraps the agent with prompt template and skill expansion.
//!
//! This is the entry point for user prompts. It handles:
//! - Loading and caching prompt templates and skills
//! - Expanding /template commands and /skill:name commands before sending to the agent
//! - Building the system prompt from tools, skills, and project context
//!
//! Mirrors the expansion logic in `@earendil-works/pi-coding-agent/src/core/agent-session.ts`
//! without the event system, compaction, retry, or extension hooks.

use std::path::{Path, PathBuf};

use crate::agent::engine::Agent;
use crate::agent::session::session::Session;
use crate::agent::types::AgentTool;
use crate::ai::providers::{Model, ProviderApi};

use super::prompt_templates::{self, PromptTemplate};
use super::skills::{self, Skill};
use super::system_prompt::{self, BuildSystemPromptOptions, ContextFile};

/// The inputs from which a session's system prompt is derived.
///
/// Keeping these values together prevents resource reloads and context-file
/// additions from accidentally rebuilding the prompt with a partial config.
#[derive(Debug, Clone, Default)]
struct PromptState {
    cwd: PathBuf,
    include_defaults: bool,
    selected_tools: Vec<String>,
    tool_snippets: Vec<(String, String)>,
    prompt_guidelines: Vec<String>,
    custom_prompt: Option<String>,
    append_system_prompt: Option<String>,
    context_files: Vec<ContextFile>,
    skills: Vec<Skill>,
}

/// Thin session layer that wraps an agent with prompt expansion.
pub struct PromptSession {
    agent: Agent,
    agent_dir: PathBuf,
    templates: Vec<PromptTemplate>,
    prompt_state: PromptState,
}

impl PromptSession {
    /// Create a new PromptSession with the given provider, model, and tools.
    ///
    /// `agent_dir` is the config directory for global resources (e.g., `~/.pi/agent/`).
    /// `templates_dirs` and `skills_dirs` are explicit paths for templates and skills.
    /// `session` is an optional pre-configured session (e.g., JSONL-backed).
    /// If `None`, an in-memory session is used.
    /// `context_files` are pre-loaded project context files injected into the system prompt.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Box<dyn ProviderApi>,
        model: Model,
        tools: Vec<Box<dyn AgentTool>>,
        cwd: PathBuf,
        agent_dir: PathBuf,
        template_paths: Vec<PathBuf>,
        skill_paths: Vec<PathBuf>,
        include_defaults: bool,
        session: Option<Session>,
        context_files: Vec<ContextFile>,
    ) -> Self {
        let mut agent = Agent::new(provider, model);
        if let Some(s) = session {
            agent.set_session(s);
        }
        let mut selected_tools = Vec::with_capacity(tools.len());
        let mut tool_snippets = Vec::with_capacity(tools.len());
        for tool in tools {
            selected_tools.push(tool.name().to_string());
            tool_snippets.push((tool.name().to_string(), tool.description().to_string()));
            agent.add_tool(tool);
        }

        // Load templates
        let templates = prompt_templates::load_prompt_templates(prompt_templates::LoadPromptTemplatesOptions {
            cwd: cwd.clone(),
            agent_dir: agent_dir.clone(),
            prompt_paths: template_paths,
            include_defaults,
        });

        // Load skills
        let skills_result = skills::load_skills(skills::LoadSkillsOptions {
            cwd: cwd.clone(),
            agent_dir: agent_dir.clone(),
            skill_paths,
            include_defaults,
        });
        let loaded_skills = skills_result.skills;

        let prompt_state = PromptState {
            cwd: cwd.clone(),
            include_defaults,
            selected_tools,
            tool_snippets,
            context_files,
            skills: loaded_skills.clone(),
            ..Default::default()
        };

        let mut prompt_session = Self {
            agent,
            agent_dir,
            templates,
            prompt_state,
        };
        prompt_session.rebuild_system_prompt();
        prompt_session
    }

    /// Access the underlying agent for event subscription etc.
    pub fn agent(&mut self) -> &mut Agent {
        &mut self.agent
    }

    /// Consume the PromptSession and return the underlying Agent.
    pub fn into_agent(self) -> Agent {
        self.agent
    }

    /// Access the underlying session.
    pub fn session(&self) -> &Session {
        self.agent.session()
    }

    /// Set the session backing the agent (e.g., a JSONL-persisted session).
    pub fn set_session(&mut self, session: Session) {
        self.agent.set_session(session);
    }

    /// Get current templates.
    pub fn templates(&self) -> &[PromptTemplate] {
        &self.templates
    }

    /// Get current skills.
    pub fn skills(&self) -> &[Skill] {
        &self.prompt_state.skills
    }

    /// Return the currently derived system prompt.
    pub fn system_prompt(&self) -> &str {
        self.agent.system_prompt()
    }

    /// Rebuild the system prompt from the complete canonical state.
    fn rebuild_system_prompt(&mut self) {
        let prompt = system_prompt::build_system_prompt(BuildSystemPromptOptions {
            custom_prompt: self.prompt_state.custom_prompt.clone(),
            selected_tools: self.prompt_state.selected_tools.clone(),
            tool_snippets: self.prompt_state.tool_snippets.clone(),
            prompt_guidelines: self.prompt_state.prompt_guidelines.clone(),
            append_system_prompt: self.prompt_state.append_system_prompt.clone(),
            cwd: self.prompt_state.cwd.clone(),
            context_files: self.prompt_state.context_files.clone(),
            skills: self.prompt_state.skills.clone(),
        });
        self.agent.set_system_prompt(prompt);
    }

    /// Reload templates and skills from configured paths.
    /// Useful when resources change at runtime.
    pub fn reload_resources(&mut self, template_paths: Vec<PathBuf>, skill_paths: Vec<PathBuf>) {
        self.templates = prompt_templates::load_prompt_templates(prompt_templates::LoadPromptTemplatesOptions {
            cwd: self.prompt_state.cwd.clone(),
            agent_dir: self.agent_dir.clone(),
            prompt_paths: template_paths,
            include_defaults: self.prompt_state.include_defaults,
        });

        let skills_result = skills::load_skills(skills::LoadSkillsOptions {
            cwd: self.prompt_state.cwd.clone(),
            agent_dir: self.agent_dir.clone(),
            skill_paths,
            include_defaults: self.prompt_state.include_defaults,
        });
        self.prompt_state.skills = skills_result.skills;
        self.rebuild_system_prompt();
    }

    /// Send a prompt to the agent, expanding templates and skills first.
    ///
    /// 1. If the text starts with `/skill:name`, expand the skill command
    /// 2. Otherwise, if it starts with `/name`, expand the prompt template
    /// 3. Then send the expanded text to the agent
    pub async fn prompt(&mut self, text: &str) -> anyhow::Result<()> {
        let expanded = self.expand(text);
        self.agent.run(&expanded).await
    }

    /// Expand a prompt text: skill commands and template commands.
    pub fn expand(&self, text: &str) -> String {
        // First, try skill expansion (/skill:name)
        let after_skills = skills::expand_skill_command(text, &self.prompt_state.skills);

        // Then, try template expansion (/templateName)
        // But only if the text wasn't changed by skill expansion (to avoid double-expansion)

        if after_skills == text {
            prompt_templates::expand_prompt_template(text, &self.templates)
        } else {
            // For skill commands, also expand templates in the user args part
            // The skill block itself should not be re-expanded
            after_skills
        }
    }

    /// Switch the model used by the underlying agent at runtime.
    pub fn switch_model(&mut self, model: crate::ai::providers::Model) {
        self.agent.switch_model(model);
    }

    /// Get the current model from the underlying agent.
    pub fn model(&self) -> &crate::ai::providers::Model {
        self.agent.model()
    }

    /// Add a context file and rebuild the system prompt.
    pub fn add_context_file(&mut self, path: PathBuf, content: String) {
        self.prompt_state.context_files.push(ContextFile { path, content });
        self.rebuild_system_prompt();
    }

    /// Get a reference to the agent's config directory.
    pub fn agent_dir(&self) -> &Path {
        &self.agent_dir
    }

    /// Get the current working directory.
    pub fn cwd(&self) -> &Path {
        &self.prompt_state.cwd
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::mock::MockProvider;

    #[test]
    fn expand_template_in_prompt() {
        let session = create_test_session();

        // expand() should expand /templates
        let result = session.expand("/greet Alice");
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn expand_regular_text_passes_through() {
        let session = create_test_session();

        let result = session.expand("Hello, world!");
        assert_eq!(result, "Hello, world!");
    }

    fn create_test_session() -> PromptSession {
        let mock = MockProvider::text("response");
        let model = crate::ai::providers::Model {
            id: "mock",
            api: "mock",
        };
        let cwd = PathBuf::from("/tmp");
        let agent_dir = PathBuf::from("/tmp/.pi/agent");
        let mut session = PromptSession::new(
            Box::new(mock),
            model,
            vec![],
            cwd,
            agent_dir,
            vec![],
            vec![],
            false,  // no defaults
            None,   // no external session
            vec![], // no context files
        );
        // Add a template manually
        session.templates.push(PromptTemplate {
            name: "greet".into(),
            description: "Greeting".into(),
            argument_hint: None,
            content: "Hello, $1!".into(),
            source: "test".into(),
            scope: prompt_templates::ResourceScope::Path,
            file_path: PathBuf::from("/fake/greet.md"),
        });
        session
    }

    struct PromptTestTool {
        name: &'static str,
        description: &'static str,
    }

    impl crate::ai::types::Tool for PromptTestTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            self.description
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
    }

    #[async_trait::async_trait]
    impl AgentTool for PromptTestTool {
        fn label(&self) -> &str {
            self.name
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _context: crate::agent::types::ToolExecutionContext,
        ) -> anyhow::Result<crate::agent::types::AgentToolResult> {
            Ok(Default::default())
        }
    }

    fn prompt_tools() -> Vec<Box<dyn AgentTool>> {
        vec![
            Box::new(PromptTestTool {
                name: "bash",
                description: "run shell commands",
            }),
            Box::new(PromptTestTool {
                name: "read",
                description: "read files",
            }),
            Box::new(PromptTestTool {
                name: "write",
                description: "write files",
            }),
            Box::new(PromptTestTool {
                name: "edit",
                description: "edit files",
            }),
        ]
    }

    fn session_with_tools(skill_paths: Vec<PathBuf>, context_files: Vec<ContextFile>) -> PromptSession {
        PromptSession::new(
            Box::new(MockProvider::text("response")),
            crate::ai::providers::Model {
                id: "mock",
                api: "mock",
            },
            prompt_tools(),
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp/.pi/agent"),
            vec![],
            skill_paths,
            false,
            None,
            context_files,
        )
    }

    fn write_skill(path: &std::path::Path, description: &str, body: &str) {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\ndescription: {description}\n---\n\n{body}\n"),
        )
        .unwrap();
    }

    #[test]
    fn new_system_prompt_contains_all_enabled_tools() {
        let session = session_with_tools(vec![], vec![]);
        let prompt = session.system_prompt();
        for tool in ["bash", "read", "write", "edit"] {
            assert!(prompt.contains(&format!("- {tool}:")), "missing {tool}: {prompt}");
        }
    }

    #[test]
    fn initial_context_files_are_accumulated() {
        let session = session_with_tools(
            vec![],
            vec![
                ContextFile {
                    path: PathBuf::from("first.md"),
                    content: "first context".into(),
                },
                ContextFile {
                    path: PathBuf::from("second.md"),
                    content: "second context".into(),
                },
            ],
        );
        assert!(session.system_prompt().contains("first context"));
        assert!(session.system_prompt().contains("second context"));
    }

    #[test]
    fn adding_context_preserves_existing_state() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("review");
        write_skill(&skill_dir, "review skill", "review instructions");
        let initial = ContextFile {
            path: PathBuf::from("initial.md"),
            content: "initial context".into(),
        };
        let mut session = session_with_tools(vec![skill_dir], vec![initial]);

        session.add_context_file(PathBuf::from("added-one.md"), "added one".into());
        session.add_context_file(PathBuf::from("added-two.md"), "added two".into());

        let prompt = session.system_prompt();
        assert!(prompt.contains("initial context"));
        assert!(prompt.contains("added one"));
        assert!(prompt.contains("added two"));
        assert!(prompt.contains("review skill"));
        assert!(prompt.contains("- bash: run shell commands"));
        assert!(prompt.contains("- read: read files"));
    }

    #[test]
    fn reload_resources_rebuilds_prompt_without_dropping_state() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("review");
        write_skill(&skill_dir, "old description", "old skill body");
        let mut session = session_with_tools(
            vec![skill_dir.clone()],
            vec![ContextFile {
                path: PathBuf::from("context.md"),
                content: "persistent context".into(),
            }],
        );
        assert!(session.system_prompt().contains("old description"));

        write_skill(&skill_dir, "new description", "new skill body");
        session.reload_resources(vec![], vec![skill_dir]);

        let prompt = session.system_prompt();
        assert!(prompt.contains("new description"));
        assert!(prompt.contains("persistent context"));
        assert!(prompt.contains("- edit: edit files"));
        assert!(!prompt.contains("old description"));
    }
}
