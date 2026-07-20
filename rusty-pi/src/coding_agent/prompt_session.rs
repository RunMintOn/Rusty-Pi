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

/// Thin session layer that wraps an agent with prompt expansion.
pub struct PromptSession {
    agent: Agent,
    cwd: PathBuf,
    agent_dir: PathBuf,
    templates: Vec<PromptTemplate>,
    skills: Vec<Skill>,
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
        for tool in tools {
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

        // Build and set system prompt
        let prompt = system_prompt::build_system_prompt(BuildSystemPromptOptions {
            selected_tools: Vec::new(), // Will be set later via set_system_prompt with full config
            cwd: cwd.clone(),
            skills: loaded_skills.clone(),
            context_files,
            ..Default::default()
        });
        agent.set_system_prompt(prompt);

        Self {
            agent,
            cwd,
            agent_dir,
            templates,
            skills: loaded_skills,
        }
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
        &self.skills
    }

    /// Reload templates and skills from configured paths.
    /// Useful when resources change at runtime.
    pub fn reload_resources(&mut self, template_paths: Vec<PathBuf>, skill_paths: Vec<PathBuf>) {
        self.templates = prompt_templates::load_prompt_templates(prompt_templates::LoadPromptTemplatesOptions {
            cwd: self.cwd.clone(),
            agent_dir: self.agent_dir.clone(),
            prompt_paths: template_paths,
            include_defaults: true,
        });

        let skills_result = skills::load_skills(skills::LoadSkillsOptions {
            cwd: self.cwd.clone(),
            agent_dir: self.agent_dir.clone(),
            skill_paths,
            include_defaults: true,
        });
        self.skills = skills_result.skills;
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
        let after_skills = skills::expand_skill_command(text, &self.skills);

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
        let cf = ContextFile { path, content };
        // Rebuild system prompt with the new context file
        let prompt = crate::coding_agent::system_prompt::build_system_prompt(
            crate::coding_agent::system_prompt::BuildSystemPromptOptions {
                cwd: self.cwd.clone(),
                skills: self.skills.clone(),
                context_files: vec![cf],
                ..Default::default()
            },
        );
        self.agent.set_system_prompt(prompt);
    }

    /// Get a reference to the agent's config directory.
    pub fn agent_dir(&self) -> &Path {
        &self.agent_dir
    }

    /// Get the current working directory.
    pub fn cwd(&self) -> &Path {
        &self.cwd
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
}
