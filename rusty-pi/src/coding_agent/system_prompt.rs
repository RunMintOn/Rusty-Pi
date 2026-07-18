//! System prompt construction — build the system prompt with tools, guidelines, skills, and project context.
//!
//! Mirrors `@earendil-works/pi-coding-agent/src/core/system-prompt.ts`.

use std::path::PathBuf;

use super::skills::{format_skills_for_prompt, Skill};

// ── Options ─────────────────────────────────────────────────────────────────

/// Options for building the system prompt.
#[derive(Debug, Clone, Default)]
pub struct BuildSystemPromptOptions {
    /// Custom system prompt (replaces default).
    pub custom_prompt: Option<String>,
    /// Tools to include in prompt. If empty, no tool list is included.
    pub selected_tools: Vec<String>,
    /// Optional one-line tool snippets keyed by tool name.
    pub tool_snippets: Vec<(String, String)>,
    /// Additional guideline bullets appended to the default guidelines.
    pub prompt_guidelines: Vec<String>,
    /// Text to append to system prompt.
    pub append_system_prompt: Option<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Pre-loaded context files (project instructions).
    pub context_files: Vec<ContextFile>,
    /// Pre-loaded skills.
    pub skills: Vec<Skill>,
}

/// A project context file (e.g., AGENTS.md).
#[derive(Debug, Clone)]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
}

// ── Default system prompt components ────────────────────────────────────────

const DEFAULT_GUIDELINES: &[&str] = &[
    "Be concise in your responses",
    "Show file paths clearly when working with files",
];

const DOC_INTRO: &str = "\
Pi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):
- Main documentation: README.md
- Additional docs: docs/
- Examples: examples/ (extensions, custom tools, SDK)
- When reading pi docs or examples, resolve docs/... under Additional docs and examples/... under Examples, not the current working directory
- When asked about: extensions (docs/extensions.md, examples/extensions/), themes (docs/themes.md), skills (docs/skills.md), prompt templates (docs/prompt-templates.md), TUI components (docs/tui.md), keybindings (docs/keybindings.md), SDK integrations (docs/sdk.md), custom providers (docs/custom-provider.md), adding models (docs/models.md), pi packages (docs/packages.md)
- When working on pi topics, read the docs and examples, and follow .md cross-references before implementing
- Always read pi .md files completely and follow links to related docs (e.g., tui.md for TUI API details)";

// ── Build system prompt ─────────────────────────────────────────────────────

/// Build the system prompt with tools, guidelines, skills, and project context.
pub fn build_system_prompt(options: BuildSystemPromptOptions) -> String {
    let prompt_cwd = options.cwd.to_string_lossy().replace('\\', "/");

    let append_section = options
        .append_system_prompt
        .as_ref()
        .map(|s| format!("\n\n{}", s))
        .unwrap_or_default();

    let skills = &options.skills;
    let context_files = &options.context_files;

    // If custom prompt is provided, use it directly
    if let Some(custom) = &options.custom_prompt {
        let mut prompt = custom.clone();

        prompt.push_str(&append_section);

        // Append project context files
        if !context_files.is_empty() {
            prompt.push_str("\n\n<project_context>\n\n");
            prompt.push_str("Project-specific instructions and guidelines:\n\n");
            for ctx_file in context_files {
                let path_str = ctx_file.path.to_string_lossy();
                prompt.push_str(&format!(
                    "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n",
                    path_str, ctx_file.content
                ));
            }
            prompt.push_str("</project_context>\n");
        }

        // Append skills section (only if read tool is available)
        let has_read = options.selected_tools.is_empty() || options.selected_tools.contains(&"read".to_string());
        if has_read && !skills.is_empty() {
            prompt.push_str(&format_skills_for_prompt(skills));
        }

        prompt.push_str(&format!("\nCurrent working directory: {}", prompt_cwd));

        return prompt;
    }

    // Build tools list
    let tools_list = if options.selected_tools.is_empty() {
        String::from("(none)")
    } else {
        let parts: Vec<String> = options
            .selected_tools
            .iter()
            .filter_map(|name| {
                options
                    .tool_snippets
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(n, snippet)| format!("- {}: {}", n, snippet))
            })
            .collect();
        if parts.is_empty() {
            String::from("(none)")
        } else {
            parts.join("\n")
        }
    };

    // Build guidelines list
    let mut guidelines: Vec<String> = Vec::new();

    // Add file exploration guidelines if bash is available but not grep/find/ls
    let has_bash = options.selected_tools.contains(&"bash".to_string());
    let has_grep = options.selected_tools.contains(&"grep".to_string());
    let has_find = options.selected_tools.contains(&"find".to_string());
    let has_ls = options.selected_tools.contains(&"ls".to_string());

    if has_bash && !has_grep && !has_find && !has_ls {
        guidelines.push("Use bash for file operations like ls, rg, find".into());
    }

    for guideline in &options.prompt_guidelines {
        let normalized = guideline.trim();
        if !normalized.is_empty() {
            guidelines.push(normalized.to_string());
        }
    }

    // Always include these
    for g in DEFAULT_GUIDELINES {
        guidelines.push(g.to_string());
    }

    let guidelines_text = guidelines
        .iter()
        .map(|g| format!("- {}", g))
        .collect::<Vec<_>>()
        .join("\n");

    let mut prompt = format!(
        "\
You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.

Available tools:
{}

In addition to the tools above, you may have access to other custom tools depending on the project.

Guidelines:
{}

{}",
        tools_list, guidelines_text, DOC_INTRO
    );

    prompt.push_str(&append_section);

    // Append project context files
    if !context_files.is_empty() {
        prompt.push_str("\n\n<project_context>\n\n");
        prompt.push_str("Project-specific instructions and guidelines:\n\n");
        for ctx_file in context_files {
            let path_str = ctx_file.path.to_string_lossy();
            prompt.push_str(&format!(
                "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n",
                path_str, ctx_file.content
            ));
        }
        prompt.push_str("</project_context>\n");
    }

    // Append skills section (only if read tool is available)
    let has_read = options.selected_tools.is_empty() || options.selected_tools.contains(&"read".to_string());
    if has_read && !skills.is_empty() {
        prompt.push_str(&format_skills_for_prompt(skills));
    }

    prompt.push_str(&format!("\nCurrent working directory: {}", prompt_cwd));

    prompt
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skill(name: &str, desc: &str) -> Skill {
        Skill {
            name: name.into(),
            description: desc.into(),
            file_path: PathBuf::from(format!("/skills/{}/SKILL.md", name)),
            base_dir: PathBuf::from(format!("/skills/{}", name)),
            source_info: crate::coding_agent::skills::SkillSourceInfo {
                source: "test".into(),
                scope: None,
                base_dir: PathBuf::from("/skills"),
            },
            disable_model_invocation: false,
        }
    }

    #[test]
    fn default_prompt_contains_tools() {
        let prompt = build_system_prompt(BuildSystemPromptOptions {
            selected_tools: vec!["read".into(), "bash".into()],
            tool_snippets: vec![
                ("read".into(), "Read file contents".into()),
                ("bash".into(), "Execute bash commands".into()),
            ],
            cwd: PathBuf::from("/test"),
            ..Default::default()
        });

        assert!(prompt.contains("Available tools:"));
        assert!(prompt.contains("read: Read file contents"));
        assert!(prompt.contains("bash: Execute bash commands"));
        assert!(prompt.contains("Current working directory: /test"));
    }

    #[test]
    fn default_prompt_no_tools() {
        let prompt = build_system_prompt(BuildSystemPromptOptions {
            cwd: PathBuf::from("/test"),
            ..Default::default()
        });

        assert!(prompt.contains("(none)"));
    }

    #[test]
    fn custom_prompt_used_directly() {
        let prompt = build_system_prompt(BuildSystemPromptOptions {
            custom_prompt: Some("You are a specialized code reviewer.".into()),
            cwd: PathBuf::from("/test"),
            ..Default::default()
        });

        assert!(prompt.starts_with("You are a specialized code reviewer."));
        assert!(prompt.contains("Current working directory: /test"));
    }

    #[test]
    fn custom_prompt_with_skills() {
        let skills = vec![make_skill("review", "Code review skill")];
        let prompt = build_system_prompt(BuildSystemPromptOptions {
            custom_prompt: Some("Custom prompt".into()),
            selected_tools: vec!["read".into()],
            skills,
            cwd: PathBuf::from("/test"),
            ..Default::default()
        });

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("review"));
    }

    #[test]
    fn append_system_prompt_text() {
        let prompt = build_system_prompt(BuildSystemPromptOptions {
            append_system_prompt: Some("# Additional Notes\n\nExtra content.".into()),
            cwd: PathBuf::from("/test"),
            ..Default::default()
        });

        assert!(prompt.contains("# Additional Notes"));
        assert!(prompt.contains("Extra content."));
    }

    #[test]
    fn context_files_included() {
        let prompt = build_system_prompt(BuildSystemPromptOptions {
            context_files: vec![ContextFile {
                path: PathBuf::from("AGENTS.md"),
                content: "## Rules\n\nBe nice.".into(),
            }],
            cwd: PathBuf::from("/test"),
            ..Default::default()
        });

        assert!(prompt.contains("<project_context>"));
        assert!(prompt.contains("<project_instructions path=\"AGENTS.md\">"));
        assert!(prompt.contains("Be nice."));
    }

    #[test]
    fn bash_without_grep_has_file_guideline() {
        let prompt = build_system_prompt(BuildSystemPromptOptions {
            selected_tools: vec!["bash".into(), "read".into()],
            tool_snippets: vec![("bash".into(), "Run commands".into()), ("read".into(), "Read files".into())],
            cwd: PathBuf::from("/test"),
            ..Default::default()
        });

        assert!(prompt.contains("Use bash for file operations like ls, rg, find"));
    }

    #[test]
    fn bash_with_grep_omits_file_guideline() {
        let prompt = build_system_prompt(BuildSystemPromptOptions {
            selected_tools: vec!["bash".into(), "grep".into(), "find".into(), "read".into()],
            tool_snippets: vec![
                ("bash".into(), "Run commands".into()),
                ("grep".into(), "Grep files".into()),
                ("find".into(), "Find files".into()),
                ("read".into(), "Read files".into()),
            ],
            cwd: PathBuf::from("/test"),
            ..Default::default()
        });

        assert!(!prompt.contains("Use bash for file operations"));
    }

    #[test]
    fn prompt_guidelines_included() {
        let prompt = build_system_prompt(BuildSystemPromptOptions {
            selected_tools: vec!["read".into()],
            tool_snippets: vec![("read".into(), "Read files".into())],
            prompt_guidelines: vec!["Always format code with rustfmt.".into()],
            cwd: PathBuf::from("/test"),
            ..Default::default()
        });

        assert!(prompt.contains("Always format code with rustfmt."));
    }
}
