//! Prompt template system — load Markdown files as /-command templates with argument substitution.
//!
//! Mirrors `@earendil-works/pi-coding-agent/src/core/prompt-templates.ts`.

use std::fs;
use std::path::MAIN_SEPARATOR;
use std::path::{Path, PathBuf};

// ── Types ───────────────────────────────────────────────────────────────────

/// A prompt template loaded from a markdown file.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub content: String,
    pub source: String,
    pub scope: ResourceScope,
    pub file_path: PathBuf,
}

/// Where the resource lives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResourceScope {
    User,
    Project,
    Path,
}

impl ResourceScope {
    #[allow(dead_code)]
    fn as_str(&self) -> &'static str {
        match self {
            ResourceScope::User => "user",
            ResourceScope::Project => "project",
            ResourceScope::Path => "path",
        }
    }
}

// ── Command argument parsing (bash-style quoting) ───────────────────────────

/// Parse command arguments respecting quoted strings (bash-style).
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for c in args_string.chars() {
        if let Some(quote) = in_quote {
            if c == quote {
                in_quote = None;
            } else {
                current.push(c);
            }
        } else if c == '"' || c == '\'' {
            in_quote = Some(c);
        } else if c.is_ascii_whitespace() {
            if !current.is_empty() {
                args.push(current.clone());
                current.clear();
            }
        } else {
            current.push(c);
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

// ── Argument substitution ──────────────────────────────────────────────────

/// Substitute argument placeholders in template content.
///
/// Supports:
/// - `$1`, `$2`, ... for positional args
/// - `$@` and `$ARGUMENTS` for all args
/// - `${N:-default}` for positional arg N with default when missing/empty
/// - `${@:-default}` and `${ARGUMENTS:-default}` for all args with a default when empty
/// - `${@:N}` for args from Nth onwards (bash-style slicing, 1-indexed)
/// - `${@:N:L}` for L args starting from Nth
pub fn substitute_args(content: &str, args: &[String]) -> String {
    let all_args = args.join(" ");

    // We use a simple state-machine approach instead of regex to avoid adding
    // a regex dependency. The patterns we match are:
    //
    // 1. ${N:-default}  or  ${@:-default}  or  ${ARGUMENTS:-default}
    // 2. ${@:N} or ${@:N:L}
    // 3. $@ or $ARGUMENTS or $N

    let mut result = String::new();
    let chars: Vec<char> = content.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '$' && i + 1 < len {
            // Try ${...} patterns first
            if chars[i + 1] == '{' {
                let brace_start = i;
                i += 2; // skip ${
                if let Some(close) = chars[i..].iter().position(|&c| c == '}') {
                    let inner: String = chars[i..i + close].iter().collect();
                    i += close + 1; // skip }

                    // Pattern 1: ${N:-default} or ${@:-default} or ${ARGUMENTS:-default}
                    if let Some(colon_dash) = inner.find(":-") {
                        let target = &inner[..colon_dash];
                        let default_val = &inner[colon_dash + 2..];
                        let value = match target {
                            "@" | "ARGUMENTS" => {
                                if all_args.is_empty() {
                                    None
                                } else {
                                    Some(all_args.as_str())
                                }
                            }
                            _ => {
                                if let Ok(n) = target.parse::<usize>() {
                                    args.get(n.wrapping_sub(1))
                                        .filter(|s| !s.is_empty())
                                        .map(|s| s.as_str())
                                } else {
                                    None
                                }
                            }
                        };
                        result.push_str(value.unwrap_or(default_val));
                        continue;
                    }

                    // Pattern 2: ${@:N} or ${@:N:L}
                    if let Some(stripped) = inner.strip_prefix("@:") {
                        let parts: Vec<&str> = stripped.split(':').collect();
                        let start = parts[0].parse::<usize>().unwrap_or(1);
                        let start_idx = if start > 0 { start - 1 } else { 0 };
                        if parts.len() > 1 {
                            if let Ok(length) = parts[1].parse::<usize>() {
                                let slice: Vec<&str> =
                                    args.iter().skip(start_idx).take(length).map(|s| s.as_str()).collect();
                                result.push_str(&slice.join(" "));
                            } else {
                                result.push_str(&all_args);
                            }
                        } else {
                            let slice: Vec<&str> = args.iter().skip(start_idx).map(|s| s.as_str()).collect();
                            result.push_str(&slice.join(" "));
                        }
                        continue;
                    }

                    // Not a recognized pattern, keep literal
                    result.push_str(&content[brace_start..i]);
                    continue;
                } else {
                    // unclosed brace, keep literal
                    result.push('$');
                    result.push('{');
                    continue;
                }
            }

            // Pattern 3: simple $N, $@, $ARGUMENTS
            // Find how many digits/word chars follow $
            let start = i + 1;
            let mut end = start;

            // Check for $ followed by digits or a word
            if chars[start] == '@' {
                result.push_str(&all_args);
                i = start + 1;
                continue;
            }

            // Check for $ARGUMENTS
            if chars[start..].starts_with(&['A', 'R', 'G', 'U', 'M', 'E', 'N', 'T', 'S']) {
                let word_end = start + 9;
                if word_end >= len || !chars[word_end].is_alphanumeric() {
                    result.push_str(&all_args);
                    i = word_end;
                    continue;
                }
            }

            // Check for $N (digits)
            while end < len && chars[end].is_ascii_digit() {
                end += 1;
            }

            if end > start {
                let num_str: String = chars[start..end].iter().collect();
                if let Ok(n) = num_str.parse::<usize>() {
                    let idx = n.wrapping_sub(1);
                    if let Some(val) = args.get(idx) {
                        result.push_str(val);
                    }
                    i = end;
                    continue;
                }
            }

            // Not a recognized $ pattern, keep literal
            result.push('$');
            i = start;
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

// ── Frontmatter parsing ────────────────────────────────────────────────────

/// Simple frontmatter parser.
/// Expects `---\n...\n---\nbody` format.
/// Returns (frontmatter key-value pairs, body).
fn parse_frontmatter(raw: &str) -> (std::collections::HashMap<String, String>, &str) {
    let mut map = std::collections::HashMap::new();

    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---\n") && !trimmed.starts_with("---\r\n") {
        return (map, raw);
    }

    // Skip past the opening ---
    let after_opening = if let Some(stripped) = trimmed.strip_prefix("---\r\n") {
        stripped
    } else if let Some(stripped) = trimmed.strip_prefix("---\n") {
        stripped
    } else {
        return (map, raw);
    };

    // Find closing ---
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
        None => return (map, raw), // malformed frontmatter, treat as body
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

// ── Load template from file ────────────────────────────────────────────────

fn load_template_from_file(file_path: &Path, source: &str, scope: ResourceScope) -> Option<PromptTemplate> {
    let raw = fs::read_to_string(file_path).ok()?;
    let (frontmatter, body) = parse_frontmatter(&raw);

    let name = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();

    let description = frontmatter
        .get("description")
        .cloned()
        .or_else(|| {
            body.lines().find(|l| !l.trim().is_empty()).map(|l| {
                let truncated: String = l.trim().chars().take(60).collect();
                if l.trim().len() > 60 {
                    format!("{}...", truncated)
                } else {
                    truncated
                }
            })
        })
        .unwrap_or_default();

    let argument_hint = frontmatter.get("argument-hint").cloned();

    Some(PromptTemplate {
        name,
        description,
        argument_hint,
        content: body.to_string(),
        source: source.to_string(),
        scope,
        file_path: file_path.to_path_buf(),
    })
}

// ── Load templates from directory ──────────────────────────────────────────

fn load_templates_from_dir(dir: &Path, source: &str, scope: ResourceScope) -> Vec<PromptTemplate> {
    let mut templates = Vec::new();

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return templates,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("md")
            && path.is_file()
            && let Some(t) = load_template_from_file(&path, source, scope)
        {
            templates.push(t);
        }
    }

    templates
}

// ── Options for loading templates ──────────────────────────────────────────

pub struct LoadPromptTemplatesOptions {
    pub cwd: PathBuf,
    pub agent_dir: PathBuf,
    pub prompt_paths: Vec<PathBuf>,
    pub include_defaults: bool,
}

/// Check if `target` is under `root` (resolved, with separator awareness).
fn is_under_path(target: &Path, root: &Path) -> bool {
    let normalized_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let normalized_target = target.canonicalize().unwrap_or_else(|_| target.to_path_buf());
    if normalized_target == normalized_root {
        return true;
    }
    let prefix = if normalized_root.to_string_lossy().ends_with(MAIN_SEPARATOR) {
        normalized_root.to_string_lossy().to_string()
    } else {
        format!("{}{}", normalized_root.to_string_lossy(), MAIN_SEPARATOR)
    };
    normalized_target.to_string_lossy().starts_with(&prefix)
}

/// Load all prompt templates from:
/// 1. Global: agent_dir/prompts/
/// 2. Project: cwd/.pi/prompts/
/// 3. Explicit prompt paths
pub fn load_prompt_templates(options: LoadPromptTemplatesOptions) -> Vec<PromptTemplate> {
    let mut templates = Vec::new();
    let config_dir_name = ".pi";

    let global_prompts_dir = options.agent_dir.join("prompts");
    let project_prompts_dir = options.cwd.join(config_dir_name).join("prompts");

    if options.include_defaults {
        templates.extend(load_templates_from_dir(
            &global_prompts_dir,
            "local",
            ResourceScope::User,
        ));
        templates.extend(load_templates_from_dir(
            &project_prompts_dir,
            "local",
            ResourceScope::Project,
        ));
    }

    // Load explicit paths
    for raw_path in &options.prompt_paths {
        let resolved = if raw_path.is_absolute() {
            raw_path.clone()
        } else {
            options.cwd.join(raw_path)
        };

        if !resolved.exists() {
            continue;
        }

        if resolved.is_dir() {
            let (source, scope) = if is_under_path(&resolved, &global_prompts_dir) {
                ("local", ResourceScope::User)
            } else if is_under_path(&resolved, &project_prompts_dir) {
                ("local", ResourceScope::Project)
            } else {
                ("local", ResourceScope::Path)
            };
            templates.extend(load_templates_from_dir(&resolved, source, scope));
        } else if resolved.is_file() && resolved.extension().and_then(|s| s.to_str()) == Some("md") {
            let (source, scope) = if is_under_path(&resolved, &global_prompts_dir) {
                ("local", ResourceScope::User)
            } else if is_under_path(&resolved, &project_prompts_dir) {
                ("local", ResourceScope::Project)
            } else {
                ("local", ResourceScope::Path)
            };
            if let Some(t) = load_template_from_file(&resolved, source, scope) {
                templates.push(t);
            }
        }
    }

    templates
}

/// Expand a prompt template if the text matches a template name.
/// Returns the expanded content or the original text if not a template.
pub fn expand_prompt_template(text: &str, templates: &[PromptTemplate]) -> String {
    if !text.starts_with('/') {
        return text.to_string();
    }

    // Matches /templateName or /templateName args
    let space_pos = text.find(' ');
    let template_name = if let Some(pos) = space_pos {
        &text[1..pos]
    } else {
        &text[1..]
    };
    let args_string = if let Some(pos) = space_pos {
        &text[pos + 1..]
    } else {
        ""
    };

    // Check if it's a skill command (starts with /skill:) — pass through, handled by skills module
    if template_name.starts_with("skill:") {
        return text.to_string();
    }

    if let Some(template) = templates.iter().find(|t| t.name == template_name) {
        let args = parse_command_args(args_string);
        return substitute_args(&template.content, &args);
    }

    text.to_string()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── parse_command_args ──────────────────────────────────────────────

    #[test]
    fn parse_empty_string() {
        assert_eq!(parse_command_args(""), Vec::<String>::new());
    }

    #[test]
    fn parse_single_word() {
        assert_eq!(parse_command_args("hello"), vec!["hello"]);
    }

    #[test]
    fn parse_multiple_words() {
        assert_eq!(parse_command_args("hello world"), vec!["hello", "world"]);
    }

    #[test]
    fn parse_quoted_string() {
        assert_eq!(parse_command_args("\"hello world\" foo"), vec!["hello world", "foo"]);
    }

    #[test]
    fn parse_single_quoted() {
        assert_eq!(parse_command_args("'hello world' foo"), vec!["hello world", "foo"]);
    }

    #[test]
    fn parse_mixed_quotes() {
        assert_eq!(
            parse_command_args("foo \"bar baz\" 'qux'"),
            vec!["foo", "bar baz", "qux"]
        );
    }

    #[test]
    fn parse_trailing_whitespace() {
        assert_eq!(parse_command_args("  hello   world  "), vec!["hello", "world"]);
    }

    // ── substitute_args ─────────────────────────────────────────────────

    #[test]
    fn sub_positional_args() {
        let result = substitute_args("Hello $1, your $2 is ready", &["Alice".into(), "report".into()]);
        assert_eq!(result, "Hello Alice, your report is ready");
    }

    #[test]
    fn sub_all_args() {
        let result = substitute_args("Args: $@", &["a".into(), "b".into(), "c".into()]);
        assert_eq!(result, "Args: a b c");
    }

    #[test]
    fn sub_arguments_variable() {
        let result = substitute_args("Args: $ARGUMENTS", &["x".into(), "y".into()]);
        assert_eq!(result, "Args: x y");
    }

    #[test]
    fn sub_default_value_when_arg_missing() {
        let result = substitute_args("${1:-default}", &[] as &[String]);
        assert_eq!(result, "default");
    }

    #[test]
    fn sub_default_value_when_arg_empty() {
        let result = substitute_args("${1:-default}", &["".into()]);
        assert_eq!(result, "default");
    }

    #[test]
    fn sub_default_value_not_used_when_arg_present() {
        let result = substitute_args("${1:-default}", &["real".into()]);
        assert_eq!(result, "real");
    }

    #[test]
    fn sub_default_all_args_empty() {
        let result = substitute_args("${@:-nothing}", &[] as &[String]);
        assert_eq!(result, "nothing");
    }

    #[test]
    fn sub_default_all_args_present() {
        let result = substitute_args("${@:-nothing}", &["something".into()]);
        assert_eq!(result, "something");
    }

    #[test]
    fn sub_slice_from_n() {
        let result = substitute_args("${@:2}", &["a".into(), "b".into(), "c".into()]);
        assert_eq!(result, "b c");
    }

    #[test]
    fn sub_slice_n_to_l() {
        let result = substitute_args("${@:2:2}", &["a".into(), "b".into(), "c".into(), "d".into()]);
        assert_eq!(result, "b c");
    }

    #[test]
    fn sub_no_args_returns_empty() {
        let result = substitute_args("$1", &[] as &[String]);
        assert_eq!(result, "");
    }

    #[test]
    fn sub_zero_index_treated_as_one() {
        // bash convention: args start at 1, but if someone writes ${@:0}, treat as 1
        let result = substitute_args("${@:0}", &["a".into(), "b".into()]);
        assert_eq!(result, "a b");
    }

    #[test]
    fn sub_missing_arg_returns_empty() {
        let result = substitute_args("$5", &["a".into(), "b".into()]);
        assert_eq!(result, "");
    }

    #[test]
    fn sub_dollar_sign_not_followed_by_pattern() {
        let result = substitute_args("not a $ pattern", &[] as &[String]);
        assert_eq!(result, "not a $ pattern");
    }

    #[test]
    fn sub_empty_template() {
        let result = substitute_args("", &["a".into(), "b".into()]);
        assert_eq!(result, "");
    }

    // ── parse_frontmatter ───────────────────────────────────────────────

    #[test]
    fn frontmatter_no_frontmatter() {
        let (map, body) = parse_frontmatter("just body text");
        assert!(map.is_empty());
        assert_eq!(body, "just body text");
    }

    #[test]
    fn frontmatter_basic() {
        let raw = "---\nname: test\ndescription: A test template\n---\n\nTemplate body here";
        let (map, body) = parse_frontmatter(raw);
        assert_eq!(map.get("name").map(|s| s.as_str()), Some("test"));
        assert_eq!(map.get("description").map(|s| s.as_str()), Some("A test template"));
        assert!(body.contains("Template body here"));
    }

    #[test]
    fn frontmatter_missing_closing_delim() {
        let raw = "---\nname: test\nbody text";
        let (map, _body) = parse_frontmatter(raw);
        // malformed: treat whole thing as body, frontmatter empty
        assert!(map.is_empty());
    }

    // ── load_template_from_file ─────────────────────────────────────────

    #[test]
    fn load_template_from_file_with_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("my-template.md");
        let mut file = fs::File::create(&file_path).unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "description: My custom template").unwrap();
        writeln!(file, "argument-hint: <file> <message>").unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "").unwrap();
        writeln!(file, "Process file $1 with context: $2").unwrap();

        let template = load_template_from_file(&file_path, "test", ResourceScope::Path).unwrap();
        assert_eq!(template.name, "my-template");
        assert_eq!(template.description, "My custom template");
        assert_eq!(template.argument_hint.as_deref(), Some("<file> <message>"));
        assert!(template.content.contains("$1"));
    }

    #[test]
    fn load_template_from_file_no_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("simple.md");
        fs::write(&file_path, "Just some content").unwrap();

        let template = load_template_from_file(&file_path, "test", ResourceScope::Path).unwrap();
        assert_eq!(template.name, "simple");
        assert_eq!(template.description, "Just some content");
        assert!(template.argument_hint.is_none());
    }

    #[test]
    fn load_template_nonexistent_file() {
        let result = load_template_from_file(Path::new("/nonexistent/template.md"), "test", ResourceScope::Path);
        assert!(result.is_none());
    }

    // ── load_templates_from_dir ─────────────────────────────────────────

    #[test]
    fn load_templates_from_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let templates = load_templates_from_dir(dir.path(), "test", ResourceScope::Path);
        assert!(templates.is_empty());
    }

    #[test]
    fn load_templates_from_dir_with_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("first.md"), "First template").unwrap();
        fs::write(dir.path().join("second.md"), "Second template").unwrap();
        fs::write(dir.path().join("not-template.txt"), "not a template").unwrap();

        let templates = load_templates_from_dir(dir.path(), "test", ResourceScope::Path);
        assert_eq!(templates.len(), 2);
        let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"first"));
        assert!(names.contains(&"second"));
    }

    // ── expand_prompt_template ──────────────────────────────────────────

    #[test]
    fn expand_known_template() {
        let templates = vec![PromptTemplate {
            name: "greet".into(),
            description: "Greeting template".into(),
            argument_hint: None,
            content: "Hello, $1!".into(),
            source: "test".into(),
            scope: ResourceScope::Path,
            file_path: PathBuf::from("/fake/greet.md"),
        }];

        let result = expand_prompt_template("/greet Alice", &templates);
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn expand_unknown_template_returns_original() {
        let templates = vec![];
        let result = expand_prompt_template("/unknown foo bar", &templates);
        assert_eq!(result, "/unknown foo bar");
    }

    #[test]
    fn expand_non_slash_text_returns_original() {
        let templates = vec![];
        let result = expand_prompt_template("just text", &templates);
        assert_eq!(result, "just text");
    }

    #[test]
    fn expand_skill_command_passes_through() {
        let templates = vec![];
        let result = expand_prompt_template("/skill:code-review origin/main", &templates);
        // skill commands are handled by skills module, not prompt templates
        assert_eq!(result, "/skill:code-review origin/main");
    }
}
