//! Skills system — discover, validate, and format Agent Skills for inclusion in system prompts.
//!
//! Mirrors `@earendil-works/pi-coding-agent/src/core/skills.ts`.
//! Follows the Agent Skills standard: https://agentskills.io

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};

// ── Constants ───────────────────────────────────────────────────────────────

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;

// ── Types ───────────────────────────────────────────────────────────────────

/// A discovered skill.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
    pub base_dir: PathBuf,
    pub source_info: SkillSourceInfo,
    pub disable_model_invocation: bool,
}

/// Source metadata for a skill.
#[derive(Debug, Clone)]
pub struct SkillSourceInfo {
    pub source: String,
    pub scope: Option<String>,
    pub base_dir: PathBuf,
}

/// A diagnostic message for resource loading issues.
#[derive(Debug, Clone)]
pub struct ResourceDiagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum DiagnosticLevel {
    Warning,
    Collision,
}

impl ResourceDiagnostic {
    pub fn warning(message: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            level: DiagnosticLevel::Warning,
            message: message.into(),
            path: path.into(),
        }
    }

    pub fn collision(message: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            level: DiagnosticLevel::Collision,
            message: message.into(),
            path: path.into(),
        }
    }
}

/// Result of loading skills from a directory.
#[derive(Debug, Clone, Default)]
pub struct LoadSkillsResult {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

// ── Name validation ─────────────────────────────────────────────────────────

/// Validate a skill name per Agent Skills spec.
/// Returns a list of error messages (empty if valid).
pub fn validate_name(name: &str) -> Vec<String> {
    let mut errors = Vec::new();

    if name.len() > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {} characters ({})",
            MAX_NAME_LENGTH,
            name.len()
        ));
    }

    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        errors.push("name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)".into());
    }

    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".into());
    }

    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".into());
    }

    errors
}

// ── Description validation ──────────────────────────────────────────────────

/// Validate a description per Agent Skills spec.
pub fn validate_description(description: Option<&str>) -> Vec<String> {
    let mut errors = Vec::new();

    match description {
        None | Some("") => {
            errors.push("description is required".into());
        }
        Some(desc) if desc.len() > MAX_DESCRIPTION_LENGTH => {
            errors.push(format!(
                "description exceeds {} characters ({})",
                MAX_DESCRIPTION_LENGTH,
                desc.len()
            ));
        }
        _ => {}
    }

    errors
}

// ── Frontmatter parsing ─────────────────────────────────────────────────────

/// Simple frontmatter parser.
/// Returns (key-value map, body text).
fn parse_frontmatter(raw: &str) -> (HashMap<String, String>, &str) {
    let mut map = HashMap::new();

    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---\n") && !trimmed.starts_with("---\r\n") {
        return (map, raw);
    }

    let after_opening = if let Some(stripped) = trimmed.strip_prefix("---\r\n") {
        stripped
    } else if let Some(stripped) = trimmed.strip_prefix("---\n") {
        stripped
    } else {
        return (map, raw);
    };

    let close_pos = after_opening.find("\n---").or_else(|| after_opening.find("\r\n---"));

    let (frontmatter_text, body_start) = match close_pos {
        Some(pos) => {
            let fm_end = if after_opening[pos..].starts_with("\r\n---") {
                pos + 5
            } else {
                pos + 4
            };
            (&after_opening[..pos], &after_opening[fm_end..])
        }
        None => return (map, raw),
    };

    for line in frontmatter_text.lines() {
        let line = line.trim();
        if let Some(colon_pos) = line.find(':') {
            let key = line[..colon_pos].trim().to_string();
            let value = line[colon_pos + 1..].trim().to_string();
            if !key.is_empty() {
                map.insert(key, value);
            }
        }
    }

    (map, body_start)
}

// ── Load single skill from file ─────────────────────────────────────────────

fn load_skill_from_file(file_path: &Path, source: &str, base_dir: &Path) -> (Option<Skill>, Vec<ResourceDiagnostic>) {
    let mut diagnostics = Vec::new();

    let raw = match fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(e) => {
            diagnostics.push(ResourceDiagnostic::warning(
                format!("failed to parse skill file: {}", e),
                file_path,
            ));
            return (None, diagnostics);
        }
    };

    let (frontmatter, _body) = parse_frontmatter(&raw);
    let skill_dir = file_path.parent().unwrap_or(Path::new(""));
    let parent_dir_name = skill_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // Validate description
    let description = frontmatter.get("description").map(|s| s.as_str());
    let desc_errors = validate_description(description);
    for err in &desc_errors {
        diagnostics.push(ResourceDiagnostic::warning(err.clone(), file_path));
    }

    // Name from frontmatter or parent directory
    let name = frontmatter
        .get("name")
        .cloned()
        .unwrap_or_else(|| parent_dir_name.to_string());

    // Validate name
    let name_errors = validate_name(&name);
    for err in &name_errors {
        diagnostics.push(ResourceDiagnostic::warning(err.clone(), file_path));
    }

    // Don't load if description is empty
    if description.is_none() || description == Some("") {
        return (None, diagnostics);
    }

    // Check disable-model-invocation
    let disable = frontmatter
        .get("disable-model-invocation")
        .map(|v| v == "true" || v == "True" || v == "TRUE" || v == "yes")
        .unwrap_or(false);

    (
        Some(Skill {
            name,
            description: description.unwrap_or_default().to_string(),
            file_path: file_path.to_path_buf(),
            base_dir: base_dir.to_path_buf(),
            source_info: SkillSourceInfo {
                source: source.to_string(),
                scope: None,
                base_dir: base_dir.to_path_buf(),
            },
            disable_model_invocation: disable,
        }),
        diagnostics,
    )
}

// ── Ignore file handling ────────────────────────────────────────────────────

struct IgnoreMatcher {
    patterns: Vec<String>,
}

impl IgnoreMatcher {
    fn new() -> Self {
        Self {
            patterns: Vec::new(),
        }
    }

    fn add_ignore_file(&mut self, path: &Path, root_dir: &Path) {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return,
        };

        let relative_dir = path
            .parent()
            .and_then(|p| p.strip_prefix(root_dir).ok())
            .unwrap_or(Path::new(""));
        let prefix = if relative_dir.as_os_str().is_empty() {
            String::new()
        } else {
            format!("{}/", relative_dir.to_string_lossy().replace(MAIN_SEPARATOR, "/"))
        };

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let (negated, pattern) = if let Some(stripped) = trimmed.strip_prefix('!') {
                (true, stripped)
            } else {
                (false, trimmed)
            };

            let pattern = if let Some(stripped) = pattern.strip_prefix('/') {
                stripped
            } else {
                pattern
            };

            let prefixed = format!("{}{}", prefix, pattern);
            if negated {
                self.patterns.push(format!("!{}", prefixed));
            } else {
                self.patterns.push(prefixed);
            }
        }
    }

    fn is_ignored(&self, path: &str) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        // Simple glob matching: check if any pattern matches the path
        for pattern in &self.patterns {
            if Self::glob_match(path, pattern) {
                // Check for negation
                if let Some(_negated) = pattern.strip_prefix('!') {
                    return false;
                }
                return true;
            }
        }
        false
    }

    fn glob_match(path: &str, pattern: &str) -> bool {
        if let Some(stripped) = pattern.strip_prefix('!') {
            return Self::simple_glob(path, stripped);
        }
        Self::simple_glob(path, pattern)
    }

    fn simple_glob(path: &str, pattern: &str) -> bool {
        // Handle **/ patterns (match any directory depth)
        if pattern.contains("**") {
            let parts: Vec<&str> = pattern.split("**").collect();
            if parts.len() == 2 {
                let prefix = parts[0].trim_end_matches('/');
                let suffix = parts[1].trim_start_matches('/');
                if prefix.is_empty() {
                    return path.ends_with(suffix) || path == suffix;
                }
                return path.starts_with(prefix) && path.ends_with(suffix)
                    || path == pattern.trim_start_matches("**/");
            }
        }

        // Simple wildcard match: * matches anything except /
        if pattern.contains('*') {
            let re_pattern = format!(
                "^{}$",
                regex::escape(pattern).replace(r"\*", "[^/]*")
            );
            if let Ok(re) = regex::Regex::new(&re_pattern) {
                return re.is_match(path);
            }
            return false;
        }

        // Exact match
        path == pattern
    }
}

// ── Load skills from directory ──────────────────────────────────────────────

/// Recursively load skills from a directory.
///
/// Discovery rules:
/// - If a directory contains SKILL.md, treat it as a skill root and do not recurse further
/// - Otherwise, load direct .md children in the root
/// - Recurse into subdirectories to find SKILL.md
pub fn load_skills_from_dir(dir: &Path, source: &str) -> LoadSkillsResult {
    load_skills_from_dir_internal(dir, source, true, None, None)
}

fn load_skills_from_dir_internal(
    dir: &Path,
    source: &str,
    include_root_files: bool,
    ignore_matcher: Option<&mut IgnoreMatcher>,
    root_dir: Option<&Path>,
) -> LoadSkillsResult {
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();

    if !dir.exists() {
        return LoadSkillsResult { skills, diagnostics };
    }

    let root = root_dir.unwrap_or(dir);
    let mut ig = match ignore_matcher {
        Some(im) => im,
        None => &mut IgnoreMatcher::new(),
    };

    // Add ignore rules from .gitignore, .ignore, .fdignore
    for ignore_file in &[".gitignore", ".ignore", ".fdignore"] {
        ig.add_ignore_file(&dir.join(ignore_file), root);
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return LoadSkillsResult { skills, diagnostics },
    };

    let entries: Vec<_> = entries.flatten().collect();

    // First pass: look for SKILL.md
    let has_skill_md = entries.iter().any(|e| e.file_name() == "SKILL.md");

    if has_skill_md {
        for entry in &entries {
            if entry.file_name() != "SKILL.md" {
                continue;
            }
            let path = entry.path();
            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace(MAIN_SEPARATOR, "/");

            if ig.is_ignored(&rel_path) {
                continue;
            }

            let (skill, diags) = load_skill_from_file(&path, source, dir);
            if let Some(s) = skill {
                skills.push(s);
            }
            diagnostics.extend(diags);
            return LoadSkillsResult { skills, diagnostics };
        }
    }

    // Second pass: recurse into directories, and optionally load .md files
    for entry in entries {
        // Skip hidden files/dirs
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with('.')
        {
            continue;
        }

        // Skip node_modules
        if entry.file_name() == "node_modules" {
            continue;
        }

        let path = entry.path();
        let file_type = entry.file_type().ok();

        let rel_path = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace(MAIN_SEPARATOR, "/");

        let ignore_path = match file_type {
            Some(t) if t.is_dir() => format!("{}/", rel_path),
            _ => rel_path.clone(),
        };

        if ig.is_ignored(&ignore_path) {
            continue;
        }

        if file_type.is_some_and(|t| t.is_dir()) {
            let sub_result = load_skills_from_dir_internal(&path, source, false, Some(&mut ig), Some(root));
            skills.extend(sub_result.skills);
            diagnostics.extend(sub_result.diagnostics);
            continue;
        }

        if !include_root_files {
            continue;
        }

        if file_type.is_some_and(|t| t.is_file()) && path.extension().and_then(|s| s.to_str()) == Some("md") {
            let (skill, diags) = load_skill_from_file(&path, source, dir);
            if let Some(s) = skill {
                skills.push(s);
            }
            diagnostics.extend(diags);
        }
    }

    LoadSkillsResult { skills, diagnostics }
}

// ── Format skills for system prompt ─────────────────────────────────────────

/// Format skills for inclusion in a system prompt.
/// Uses XML format per Agent Skills standard.
///
/// Skills with `disable_model_invocation = true` are excluded from the prompt
/// (they can only be invoked explicitly via /skill:name commands).
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<&Skill> = skills.iter().filter(|s| !s.disable_model_invocation).collect();

    if visible.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        String::new(),
        "The following skills provide specialized instructions for specific tasks.".into(),
        "Use the read tool to load a skill's file when the task matches its description.".into(),
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".into(),
        String::new(),
        "<available_skills>".into(),
    ];

    for skill in &visible {
        lines.push("  <skill>".into());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!("    <description>{}</description>", escape_xml(&skill.description)));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.file_path.to_string_lossy())
        ));
        lines.push("  </skill>".into());
    }

    lines.push("</available_skills>".into());

    lines.join("\n")
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ── Load skills with options ────────────────────────────────────────────────

pub struct LoadSkillsOptions {
    pub cwd: PathBuf,
    pub agent_dir: PathBuf,
    pub skill_paths: Vec<PathBuf>,
    pub include_defaults: bool,
}

/// Load skills from all configured locations.
/// Returns skills and any validation/compatibility diagnostics.
pub fn load_skills(options: LoadSkillsOptions) -> LoadSkillsResult {
    let config_dir_name = ".pi";
    let mut all_diagnostics = Vec::new();
    let mut skill_map: HashMap<String, Skill> = HashMap::new();
    let mut real_path_set: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut collision_diagnostics = Vec::new();

    let add_skills = |result: LoadSkillsResult,
                      skill_map: &mut HashMap<String, Skill>,
                      real_path_set: &mut std::collections::HashSet<PathBuf>,
                      all_diagnostics: &mut Vec<ResourceDiagnostic>,
                      collision_diagnostics: &mut Vec<ResourceDiagnostic>| {
        all_diagnostics.extend(result.diagnostics);
        for skill in result.skills {
            // Canonicalize to detect duplicates via symlink
            let real_path = skill.file_path.canonicalize().unwrap_or_else(|_| skill.file_path.clone());

            if real_path_set.contains(&real_path) {
                continue;
            }

            if let Some(_existing) = skill_map.get(&skill.name) {
                collision_diagnostics.push(ResourceDiagnostic::collision(
                    format!("name \"{}\" collision", skill.name),
                    &skill.file_path,
                ));
                // Keep the existing one, skip this one
            } else {
                skill_map.insert(skill.name.clone(), skill);
                real_path_set.insert(real_path);
            }
        }
    };

    if options.include_defaults {
        let user_skills_dir = options.agent_dir.join("skills");
        let project_skills_dir = options
            .cwd
            .join(config_dir_name)
            .join("skills");

        let result = load_skills_from_dir_internal(&user_skills_dir, "user", true, None, None);
        add_skills(
            result,
            &mut skill_map,
            &mut real_path_set,
            &mut all_diagnostics,
            &mut collision_diagnostics,
        );

        let result = load_skills_from_dir_internal(&project_skills_dir, "project", true, None, None);
        add_skills(
            result,
            &mut skill_map,
            &mut real_path_set,
            &mut all_diagnostics,
            &mut collision_diagnostics,
        );
    }

    let user_skills_dir = options.agent_dir.join("skills");
    let project_skills_dir = options.cwd.join(config_dir_name).join("skills");

    let is_under_path = |target: &Path, root: &Path| -> bool {
        let Ok(normalized_target) = target.canonicalize() else {
            return false;
        };
        let Ok(normalized_root) = root.canonicalize() else {
            return false;
        };
        if normalized_target == normalized_root {
            return true;
        }
        let prefix = format!("{}{}", normalized_root.to_string_lossy(), MAIN_SEPARATOR);
        normalized_target.to_string_lossy().starts_with(&prefix)
    };

    for raw_path in &options.skill_paths {
        let resolved = if raw_path.is_absolute() {
            raw_path.clone()
        } else {
            options.cwd.join(raw_path)
        };

        if !resolved.exists() {
            all_diagnostics.push(ResourceDiagnostic::warning(
                "skill path does not exist",
                &resolved,
            ));
            continue;
        }

        let source = if !options.include_defaults && is_under_path(&resolved, &user_skills_dir) {
            "user"
        } else if !options.include_defaults && is_under_path(&resolved, &project_skills_dir) {
            "project"
        } else {
            "path"
        };

        if resolved.is_dir() {
            let result = load_skills_from_dir_internal(&resolved, source, true, None, None);
            add_skills(
                result,
                &mut skill_map,
                &mut real_path_set,
                &mut all_diagnostics,
                &mut collision_diagnostics,
            );
        } else if resolved.is_file() && resolved.extension().and_then(|s| s.to_str()) == Some("md") {
            let (skill, diags) = load_skill_from_file(&resolved, source, resolved.parent().unwrap_or(Path::new("")));
            all_diagnostics.extend(diags);
            if let Some(s) = skill {
                let real_path = s.file_path.canonicalize().unwrap_or_else(|_| s.file_path.clone());
                if !real_path_set.contains(&real_path) {
                    if let Some(_existing) = skill_map.get(&s.name) {
                        collision_diagnostics.push(ResourceDiagnostic::collision(
                            format!("name \"{}\" collision", s.name),
                            &s.file_path,
                        ));
                    } else {
                        skill_map.insert(s.name.clone(), s);
                        real_path_set.insert(real_path);
                    }
                }
            }
        } else {
            all_diagnostics.push(ResourceDiagnostic::warning(
                "skill path is not a markdown file",
                &resolved,
            ));
        }
    }

    LoadSkillsResult {
        skills: skill_map.into_values().collect(),
        diagnostics: {
            let mut all = all_diagnostics;
            all.extend(collision_diagnostics);
            all
        },
    }
}

/// Expand a skill command (/skill:name args) to its full content.
/// Returns the expanded text, or the original text if not a skill command.
pub fn expand_skill_command(text: &str, skills: &[Skill]) -> String {
    if !text.starts_with("/skill:") {
        return text.to_string();
    }

    let rest = &text[7..]; // skip "/skill:"
    let space_pos = rest.find(' ');
    let skill_name = if let Some(pos) = space_pos {
        &rest[..pos]
    } else {
        rest
    };
    let args = if let Some(pos) = space_pos {
        rest[pos + 1..].trim()
    } else {
        ""
    };

    let skill = skills.iter().find(|s| s.name == skill_name);
    match skill {
        Some(skill) => {
            let content = match fs::read_to_string(&skill.file_path) {
                Ok(c) => c,
                Err(_) => return text.to_string(),
            };
            let (_frontmatter, body) = parse_frontmatter(&content);
            let body = body.trim();

            let skill_block = format!(
                "<skill name=\"{}\" location=\"{}\">\n\
                 References are relative to {}.\n\n\
                 {}\n\
                 </skill>",
                skill.name,
                skill.file_path.to_string_lossy(),
                skill.base_dir.to_string_lossy(),
                body,
            );

            if !args.is_empty() {
                format!("{}\n\n{}", skill_block, args)
            } else {
                skill_block
            }
        }
        None => text.to_string(),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── validate_name ───────────────────────────────────────────────────

    #[test]
    fn valid_name_passes() {
        let errors = validate_name("my-skill");
        assert!(errors.is_empty());
    }

    #[test]
    fn valid_name_with_numbers() {
        let errors = validate_name("skill-42");
        assert!(errors.is_empty());
    }

    #[test]
    fn name_too_long() {
        let long = "a".repeat(MAX_NAME_LENGTH + 1);
        let errors = validate_name(&long);
        assert!(errors.iter().any(|e| e.contains("exceeds")));
    }

    #[test]
    fn name_with_uppercase_fails() {
        let errors = validate_name("MySkill");
        assert!(errors.iter().any(|e| e.contains("invalid characters")));
    }

    #[test]
    fn name_starting_with_hyphen_fails() {
        let errors = validate_name("-skill");
        assert!(errors.iter().any(|e| e.contains("start or end")));
    }

    #[test]
    fn name_with_consecutive_hyphens_fails() {
        let errors = validate_name("my--skill");
        assert!(errors.iter().any(|e| e.contains("consecutive hyphens")));
    }

    // ── validate_description ────────────────────────────────────────────

    #[test]
    fn valid_description_passes() {
        let errors = validate_description(Some("A useful skill"));
        assert!(errors.is_empty());
    }

    #[test]
    fn missing_description_fails() {
        let errors = validate_description(None);
        assert!(errors.iter().any(|e| e.contains("required")));
    }

    #[test]
    fn empty_description_fails() {
        let errors = validate_description(Some(""));
        assert!(errors.iter().any(|e| e.contains("required")));
    }

    #[test]
    fn description_too_long() {
        let long = "a".repeat(MAX_DESCRIPTION_LENGTH + 1);
        let errors = validate_description(Some(&long));
        assert!(errors.iter().any(|e| e.contains("exceeds")));
    }

    // ── format_skills_for_prompt ────────────────────────────────────────

    fn make_skill(name: &str, desc: &str, disabled: bool) -> Skill {
        Skill {
            name: name.into(),
            description: desc.into(),
            file_path: PathBuf::from(format!("/skills/{}/SKILL.md", name)),
            base_dir: PathBuf::from(format!("/skills/{}", name)),
            source_info: SkillSourceInfo {
                source: "test".into(),
                scope: None,
                base_dir: PathBuf::from("/skills"),
            },
            disable_model_invocation: disabled,
        }
    }

    #[test]
    fn format_no_skills() {
        let result = format_skills_for_prompt(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn format_single_skill() {
        let skills = vec![make_skill("test-skill", "A test skill", false)];
        let result = format_skills_for_prompt(&skills);
        assert!(result.contains("<available_skills>"));
        assert!(result.contains("<name>test-skill</name>"));
        assert!(result.contains("<description>A test skill</description>"));
        assert!(result.contains("</available_skills>"));
        // default instructions should be present
        assert!(result.contains("The following skills provide specialized instructions"));
    }

    #[test]
    fn format_excludes_disabled_skills() {
        let skills = vec![
            make_skill("visible", "I am visible", false),
            make_skill("hidden", "I am hidden", true),
        ];
        let result = format_skills_for_prompt(&skills);
        assert!(result.contains("visible"));
        assert!(!result.contains("hidden"));
    }

    #[test]
    fn xml_escapes_special_chars() {
        let skills = vec![
            make_skill("test&skill", "description with < & > \" '", false),
        ];
        let result = format_skills_for_prompt(&skills);
        assert!(result.contains("test&amp;skill"));
        assert!(result.contains("&lt;"));
        assert!(result.contains("&gt;"));
        assert!(result.contains("&quot;"));
        assert!(result.contains("&apos;"));
    }

    // ── load_skill_from_file ────────────────────────────────────────────

    #[test]
    fn load_skill_file_with_valid_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        fs::create_dir(&skill_dir).unwrap();
        let file_path = skill_dir.join("SKILL.md");
        let mut file = fs::File::create(&file_path).unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "name: my-skill").unwrap();
        writeln!(file, "description: A test skill for testing").unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "").unwrap();
        writeln!(file, "## Usage").unwrap();
        writeln!(file, "Do something cool").unwrap();

        let (skill, diags) = load_skill_from_file(&file_path, "test", &skill_dir);
        assert!(diags.is_empty(), "unexpected diagnostics: {:?}", diags);
        let skill = skill.expect("should load successfully");
        assert_eq!(skill.name, "my-skill");
        assert_eq!(skill.description, "A test skill for testing");
    }

    #[test]
    fn load_skill_file_no_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("SKILL.md");
        fs::write(&file_path, "Just content, no frontmatter").unwrap();

        let (skill, diags) = load_skill_file_from_dir(file_path, "test");
        assert!(!diags.is_empty());
        assert!(skill.is_none());
    }

    // Helper for the test above
    fn load_skill_file_from_dir(file_path: PathBuf, source: &str) -> (Option<Skill>, Vec<ResourceDiagnostic>) {
        let parent = file_path.parent().unwrap_or(Path::new(""));
        load_skill_from_file(&file_path, source, parent)
    }

    #[test]
    fn load_skill_with_disable_model_invocation() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("hidden-skill");
        fs::create_dir(&skill_dir).unwrap();
        let file_path = skill_dir.join("SKILL.md");
        let mut file = fs::File::create(&file_path).unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "name: hidden-skill").unwrap();
        writeln!(file, "description: A hidden skill").unwrap();
        writeln!(file, "disable-model-invocation: true").unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "").unwrap();
        writeln!(file, "Hidden content").unwrap();

        let (skill, _diags) = load_skill_from_file(&file_path, "test", &skill_dir);
        let skill = skill.unwrap();
        assert!(skill.disable_model_invocation);
    }

    // ── load_skills_from_dir ────────────────────────────────────────────

    #[test]
    fn load_skills_from_dir_with_skill_md() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        fs::create_dir(&skill_dir).unwrap();
        let mut file = fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "description: A test skill").unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "").unwrap();
        writeln!(file, "Skill body").unwrap();

        let result = load_skills_from_dir(dir.path(), "test");
        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].name, "my-skill");
    }

    #[test]
    fn load_skills_from_dir_flat_md_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = fs::File::create(dir.path().join("simple-skill.md")).unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "description: A flat skill").unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "Body").unwrap();

        let result = load_skills_from_dir(dir.path(), "test");
        assert_eq!(result.skills.len(), 1);
    }

    // ── expand_skill_command ────────────────────────────────────────────

    #[test]
    fn expand_skill_command_known_skill() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        fs::create_dir(&skill_dir).unwrap();
        let file_path = skill_dir.join("SKILL.md");
        let mut file = fs::File::create(&file_path).unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "description: A test skill").unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "").unwrap();
        writeln!(file, "## Instructions").unwrap();
        writeln!(file, "Do the thing").unwrap();

        let skills = vec![Skill {
            name: "my-skill".into(),
            description: "A test skill".into(),
            file_path,
            base_dir: skill_dir,
            source_info: SkillSourceInfo {
                source: "test".into(),
                scope: None,
                base_dir: dir.path().to_path_buf(),
            },
            disable_model_invocation: false,
        }];

        let result = expand_skill_command("/skill:my-skill do this", &skills);
        assert!(result.contains("<skill name=\"my-skill\""));
        assert!(result.contains("## Instructions"));
        assert!(result.contains("do this")); // user args appended
    }

    #[test]
    fn expand_skill_command_unknown_passes_through() {
        let result = expand_skill_command("/skill:unknown args", &[]);
        assert_eq!(result, "/skill:unknown args");
    }

    #[test]
    fn expand_skill_command_non_skill_passes_through() {
        let result = expand_skill_command("just regular text", &[]);
        assert_eq!(result, "just regular text");
    }
}
