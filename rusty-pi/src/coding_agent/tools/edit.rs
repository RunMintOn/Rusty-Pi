//! Edit tool — apply exact-text replacements to files.
//!
//! Mirrors the original `@earendil-works/pi-coding-agent/src/core/tools/edit.ts`
//! and `edit-diff.ts`.

use crate::agent::types::{AgentTool, AgentToolResult};
use crate::ai::types::{Content, Tool};
use crate::coding_agent::tools::write::with_file_mutation_queue;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

// ── Parameters ──────────────────────────────────────────────────────────────

/// A single text replacement.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Edit {
    /// Exact text to search for. Must be unique in the file.
    #[serde(rename = "oldText")]
    pub old_text: String,
    /// Replacement text.
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// Parameters for the edit tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EditParams {
    /// Path to the file to edit.
    pub path: String,
    /// One or more targeted replacements.
    pub edits: Vec<Edit>,
}

// ── Unicode / text normalization ────────────────────────────────────────────

/// Strip UTF-8 BOM if present.
pub fn strip_bom(content: &str) -> (String, String) {
    if let Some(rest) = content.strip_prefix('\u{FEFF}') {
        ("\u{FEFF}".to_string(), rest.to_string())
    } else {
        (String::new(), content.to_string())
    }
}

/// Detect line ending style.
pub fn detect_line_ending(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Normalize line endings to LF.
pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Restore line endings to the original style.
pub fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

/// Normalize text for fuzzy matching:
/// - Strip trailing whitespace per line
/// - Normalize smart quotes to ASCII
/// - Normalize dashes/hyphens to ASCII hyphen
/// - Normalize special spaces to regular space
pub fn normalize_for_fuzzy_match(text: &str) -> String {
    let result = text
        .chars()
        .map(|c| match c {
            // Smart single quotes → '
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            // Smart double quotes → "
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            // Dashes/hyphens → -
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}' | '\u{2212}' => '-',
            // Special spaces → regular space
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect::<String>();

    // Strip trailing whitespace from each line
    let lines: Vec<&str> = result.split('\n').collect();
    let trimmed: Vec<&str> = lines.iter().map(|l| l.trim_end()).collect();
    trimmed.join("\n")
}

/// Fuzzy-find oldText in content. Returns (found, index, match_length, used_fuzzy).
fn fuzzy_find_text(content: &str, old_text: &str) -> FuzzyMatch {
    // Try exact match first
    if let Some(idx) = content.find(old_text) {
        return FuzzyMatch {
            found: true,
            index: idx,
            match_length: old_text.len(),
            used_fuzzy: false,
        };
    }

    // Try fuzzy match
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old = normalize_for_fuzzy_match(old_text);
    if let Some(idx) = fuzzy_content.find(&fuzzy_old) {
        return FuzzyMatch {
            found: true,
            index: idx,
            match_length: fuzzy_old.len(),
            used_fuzzy: true,
        };
    }

    FuzzyMatch {
        found: false,
        index: 0,
        match_length: 0,
        used_fuzzy: false,
    }
}

#[derive(Debug, Clone)]
struct FuzzyMatch {
    found: bool,
    index: usize,
    match_length: usize,
    used_fuzzy: bool,
}

fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old = normalize_for_fuzzy_match(old_text);
    fuzzy_content.matches(&fuzzy_old).count()
}

// ── Apply edits ─────────────────────────────────────────────────────────────

/// Apply replacements in reverse order to keep offsets stable.
fn apply_replacements(content: &str, replacements: &[(usize, usize, &str)]) -> String {
    let mut result = content.to_string();
    // Sort by index descending and apply
    let mut sorted: Vec<_> = replacements.iter().enumerate().collect();
    sorted.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));

    for (_, (idx, len, new_text)) in &sorted {
        let range = *idx..*idx + len;
        result.replace_range(range, new_text);
    }
    result
}

/// Apply edits to normalized (LF) content.
fn apply_edits_to_normalized_content(
    content: &str,
    edits: &[Edit],
    path: &str,
) -> Result<AppliedEdits, String> {
    if edits.is_empty() {
        return Err("edits must contain at least one replacement.".to_string());
    }

    // Validate no empty oldText
    for (i, edit) in edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            return Err(get_empty_old_text_error(path, i, edits.len()));
        }
    }

    // Normalize edit text to LF
    let normalized_edits: Vec<Edit> = edits
        .iter()
        .map(|e| Edit {
            old_text: normalize_to_lf(&e.old_text),
            new_text: normalize_to_lf(&e.new_text),
        })
        .collect();

    // Try exact matching first; if any fails, try fuzzy matching
    let initial_matches: Vec<FuzzyMatch> = normalized_edits
        .iter()
        .map(|e| fuzzy_find_text(content, &e.old_text))
        .collect();

    let used_fuzzy = initial_matches.iter().any(|m| m.used_fuzzy);
    let replacement_base = if used_fuzzy {
        normalize_for_fuzzy_match(content)
    } else {
        content.to_string()
    };

    // Match and validate each edit
    let mut matched: Vec<(usize, usize, String)> = Vec::new(); // (index, length, new_text)
    for (i, edit) in normalized_edits.iter().enumerate() {
        let match_result = fuzzy_find_text(&replacement_base, &edit.old_text);
        if !match_result.found {
            return Err(get_not_found_error(path, i, edits.len()));
        }

        let occurrences = count_occurrences(&replacement_base, &edit.old_text);
        if occurrences > 1 {
            return Err(get_duplicate_error(path, i, edits.len(), occurrences));
        }

        matched.push((match_result.index, match_result.match_length, edit.new_text.clone()));
    }

    // Check for overlaps
    matched.sort_by(|a, b| a.0.cmp(&b.0));
    for i in 1..matched.len() {
        let prev_end = matched[i - 1].0 + matched[i - 1].1;
        if prev_end > matched[i].0 {
            let prev_idx = i - 1;
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {}. Merge them into one edit or target disjoint regions.",
                prev_idx, i, path
            ));
        }
    }

    let base_content = content.to_string();
    let replacements: Vec<(usize, usize, &str)> = matched
        .iter()
        .map(|(idx, len, text)| (*idx, *len, text.as_str()))
        .collect();
    let new_content = apply_replacements(&replacement_base, &replacements);

    if base_content == new_content {
        return Err(get_no_change_error(path, edits.len()));
    }

    Ok(AppliedEdits {
        base_content,
        new_content,
    })
}

struct AppliedEdits {
    base_content: String,
    new_content: String,
}

// ── Error messages ──────────────────────────────────────────────────────────

fn get_not_found_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        format!(
            "Could not find the exact text in {}. The old text must match exactly including all whitespace and newlines.",
            path
        )
    } else {
        format!(
            "Could not find edits[{}] in {}. The oldText must match exactly including all whitespace and newlines.",
            index, path
        )
    }
}

fn get_duplicate_error(path: &str, index: usize, total: usize, occurrences: usize) -> String {
    if total == 1 {
        format!(
            "Found {} occurrences of the text in {}. The text must be unique. Please provide more context to make it unique.",
            occurrences, path
        )
    } else {
        format!(
            "Found {} occurrences of edits[{}] in {}. Each oldText must be unique. Please provide more context to make it unique.",
            occurrences, index, path
        )
    }
}

fn get_empty_old_text_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        format!("oldText must not be empty in {}.", path)
    } else {
        format!("edits[{}].oldText must not be empty in {}.", index, path)
    }
}

fn get_no_change_error(path: &str, total: usize) -> String {
    if total == 1 {
        format!(
            "No changes made to {}. The replacement produced identical content. \
             This might indicate an issue with special characters or the text not existing as expected.",
            path
        )
    } else {
        format!("No changes made to {}. The replacements produced identical content.", path)
    }
}

// ── Diff generation ─────────────────────────────────────────────────────────

/// Generate a display-oriented diff string with line numbers.
pub fn generate_diff_string(old_content: &str, new_content: &str) -> (String, Option<usize>) {
    use similar::TextDiff;
    let diff = TextDiff::from_lines(old_content, new_content);

    let old_lines: Vec<&str> = old_content.split('\n').collect();
    let new_lines: Vec<&str> = new_content.split('\n').collect();
    let max_line = old_lines.len().max(new_lines.len());
    let width = max_line.to_string().len();

    let mut output = Vec::new();
    let mut first_changed_line: Option<usize> = None;

    for change in diff.iter_all_changes() {
        let idx = change.old_index().or_else(|| change.new_index());
        let line_num = idx.map(|i| i + 1).unwrap_or(0);
        match change.tag() {
            similar::ChangeTag::Equal => {
                if let Some(_old_idx) = change.old_index() {
                    output.push(format!(" {:>width$} {}", line_num, change.value().trim_end()));
                }
            }
            similar::ChangeTag::Delete => {
                if let Some(old_idx) = change.old_index() {
                    output.push(format!("-{:>width$} {}", old_idx + 1, change.value().trim_end()));
                }
            }
            similar::ChangeTag::Insert => {
                if let Some(new_idx) = change.new_index() {
                    if first_changed_line.is_none() {
                        first_changed_line = Some(new_idx + 1);
                    }
                    output.push(format!("+{:>width$} {}", new_idx + 1, change.value().trim_end()));
                }
            }
        }
    }

    (output.join("\n"), first_changed_line)
}

/// Generate a standard unified patch.
pub fn generate_unified_patch(path: &str, old_content: &str, new_content: &str) -> String {
    use similar::TextDiff;
    let diff = TextDiff::from_lines(old_content, new_content);

    let mut result = format!("--- {}\n+++ {}\n", path, path);

    for group in diff.grouped_ops(4) {
        let first = group.first().unwrap();
        let last = group.last().unwrap();
        let old_range = first.old_range();
        let new_range = first.new_range();
        let last_old_range = last.old_range();
        let last_new_range = last.new_range();

        let old_start = old_range.start + 1;
        let old_len = if last_old_range.end == old_range.end {
            old_range.len()
        } else {
            last_old_range.end - old_range.start
        };
        let new_start = new_range.start + 1;
        let new_len = if last_new_range.end == new_range.end {
            new_range.len()
        } else {
            last_new_range.end - new_range.start
        };

        result.push_str(&format!("@@ -{},{} +{},{} @@\n", old_start, old_len, new_start, new_len));

        for op in &group {
            for change in diff.iter_changes(op) {
                let sign = match change.tag() {
                    similar::ChangeTag::Equal => " ",
                    similar::ChangeTag::Delete => "-",
                    similar::ChangeTag::Insert => "+",
                };
                result.push_str(&format!("{}{}\n", sign, change.value().trim_end()));
            }
        }
    }

    result
}

// ── Prepare arguments (legacy compat) ───────────────────────────────────────

/// Shim for models that send single oldText/newText instead of edits array,
/// or send edits as a JSON string.
fn prepare_edit_arguments(params: &serde_json::Value) -> Result<EditParams, String> {
    let mut args: serde_json::Value = params.clone();

    // If edits is a JSON string, parse it
    if let Some(edits_str) = args.get("edits").and_then(|v| v.as_str())
        && let Ok(parsed) = serde_json::from_str::<Vec<Edit>>(edits_str)
        && let Some(obj) = args.as_object_mut() {
                obj.insert("edits".to_string(), serde_json::to_value(parsed).unwrap());
        }

    // Try standard deserialization
    if let Ok(p) = serde_json::from_value::<EditParams>(args.clone())
        && !p.edits.is_empty() {
            return Ok(p);
        }

    // Legacy: wrap single oldText/newText into edits array
    let old_text = args.get("oldText").and_then(|v| v.as_str());
    let new_text = args.get("newText").and_then(|v| v.as_str());
    if let (Some(old), Some(new)) = (old_text, new_text) {
        return Ok(EditParams {
            path: args.get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            edits: vec![Edit {
                old_text: old.to_string(),
                new_text: new.to_string(),
            }],
        });
    }

    Err("Invalid edit parameters: must have 'edits' array or legacy 'oldText'/'newText' fields.".to_string())
}

// ── Tool implementation ─────────────────────────────────────────────────────

/// The edit tool — applies exact text replacements to files.
pub struct EditTool {
    shared_cwd: Arc<RwLock<PathBuf>>,
}

impl EditTool {
    pub fn new(shared_cwd: Arc<RwLock<PathBuf>>) -> Self {
        Self { shared_cwd }
    }

    fn cwd(&self) -> String {
        self.shared_cwd.read().expect("shared_cwd lock poisoned").to_string_lossy().to_string()
    }
}

impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit a single file using exact text replacement. \
Every edits[].oldText must match a unique, non-overlapping region of the original file. \
If two changes affect the same block or nearby lines, merge them into one edit instead of \
emitting overlapping edits. Do not include large unchanged regions just to connect distant changes."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit (relative or absolute)"
                },
                "edits": {
                    "type": "array",
                    "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": {
                                "type": "string",
                                "description": "Exact text to replace. Must be unique in the original file."
                            },
                            "newText": {
                                "type": "string",
                                "description": "Replacement text."
                            }
                        },
                        "required": ["oldText", "newText"]
                    }
                }
            },
            "required": ["path", "edits"]
        })
    }
}

#[async_trait]
impl AgentTool for EditTool {
    fn label(&self) -> &str {
        "edit"
    }

    fn prepare_arguments(&self, args: serde_json::Value) -> serde_json::Value {
        match prepare_edit_arguments(&args) {
            Ok(params) => serde_json::to_value(params).unwrap_or(args),
            Err(_) => args,
        }
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> anyhow::Result<AgentToolResult> {
        let edit_params: EditParams = prepare_edit_arguments(&params)
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        let absolute_path = if Path::new(&edit_params.path).is_absolute() {
            edit_params.path.clone()
        } else {
            Path::new(&self.cwd()).join(&edit_params.path).to_string_lossy().to_string()
        };
        let path_buf = Path::new(&absolute_path).to_path_buf();

        with_file_mutation_queue(&absolute_path, || async {
            // Check abort
            if let Some(rx) = &signal
                && *rx.borrow() {
                    return Ok(AgentToolResult {
                        content: vec![Content::Text { text: "Operation aborted".into() }],
                        details: serde_json::json!({"aborted": true}),
                        ..Default::default()
                    });
                }

            // Check file exists
            if !path_buf.exists() {
                anyhow::bail!("Could not edit file: {}. File not found.", edit_params.path);
            }

            // Read file
            let raw_content = tokio::fs::read_to_string(&path_buf).await
                .map_err(|e| anyhow::anyhow!("Failed to read '{}': {}", edit_params.path, e))?;

            // Check abort after IO
            if let Some(rx) = &signal
                && *rx.borrow() {
                    return Ok(AgentToolResult {
                        content: vec![Content::Text { text: "Operation aborted".into() }],
                        details: serde_json::json!({"aborted": true}),
                        ..Default::default()
                    });
                }

            // Strip BOM
            let (bom, content) = strip_bom(&raw_content);
            let original_ending = detect_line_ending(&content);
            let normalized = normalize_to_lf(&content);

            // Apply edits
            let applied = apply_edits_to_normalized_content(&normalized, &edit_params.edits, &edit_params.path)
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            // Restore line endings and BOM
            let final_content = bom + &restore_line_endings(&applied.new_content, original_ending);

            // Write
            tokio::fs::write(&path_buf, &final_content).await
                .map_err(|e| anyhow::anyhow!("Failed to write '{}': {}", edit_params.path, e))?;

            // Generate diff
            let (diff, first_changed_line) = generate_diff_string(
                &applied.base_content,
                &applied.new_content,
            );
            let patch = generate_unified_patch(
                &edit_params.path,
                &applied.base_content,
                &applied.new_content,
            );

            Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: format!("Successfully replaced {} block(s) in {}.", edit_params.edits.len(), edit_params.path),
                }],
                details: serde_json::json!({
                    "diff": diff,
                    "patch": patch,
                    "first_changed_line": first_changed_line,
                }),
                ..Default::default()
            })
        }).await
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    async fn tool_and_temp() -> (EditTool, Arc<TokioMutex<tempfile::TempDir>>) {
        let tmp = tempfile::tempdir().unwrap();
        let shared_cwd = Arc::new(RwLock::new(tmp.path().to_path_buf()));
        let tool = EditTool::new(shared_cwd);
        let tmp_arc = Arc::new(TokioMutex::new(tmp));
        (tool, tmp_arc)
    }

    #[tokio::test]
    async fn edit_single_replacement() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        // Create a test file
        let file_path = Path::new(&dir).join("test.txt");
        tokio::fs::write(&file_path, "hello world\nfoo bar\n").await.unwrap();

        let result = tool
            .execute("c1", serde_json::json!({
                "path": "test.txt",
                "edits": [{"oldText": "foo bar", "newText": "baz qux"}]
            }), None)
            .await
            .unwrap();

        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("Successfully replaced"), "Got: {}", text);

        // Verify content
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "hello world\nbaz qux\n");
    }

    #[tokio::test]
    async fn edit_multiple_disjoint_replacements() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        let file_path = Path::new(&dir).join("multi.txt");
        tokio::fs::write(&file_path, "line one\nline two\nline three\n").await.unwrap();

        let result = tool
            .execute("c2", serde_json::json!({
                "path": "multi.txt",
                "edits": [
                    {"oldText": "line one", "newText": "changed one"},
                    {"oldText": "line three", "newText": "changed three"}
                ]
            }), None)
            .await
            .unwrap();

        let text = match &result.content[0] { Content::Text { text } => text.as_str(), _ => panic!("Expected text content"), };
        assert!(text.contains("2 block(s)"), "Got: {}", text);

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "changed one\nline two\nchanged three\n");
    }

    #[tokio::test]
    async fn edit_not_found() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        let file_path = Path::new(&dir).join("nope.txt");
        tokio::fs::write(&file_path, "existing content\n").await.unwrap();

        let result = tool
            .execute("c3", serde_json::json!({
                "path": "nope.txt",
                "edits": [{"oldText": "nonexistent text", "newText": "replacement"}]
            }), None)
            .await;

        assert!(result.is_err(), "Should error when oldText not found");
        let err = result.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("Could not find"), "Got: {}", msg);
    }

    #[tokio::test]
    async fn edit_duplicate_old_text() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        let file_path = Path::new(&dir).join("dup.txt");
        tokio::fs::write(&file_path, "repeat\nother\nrepeat\n").await.unwrap();

        let result = tool
            .execute("c4", serde_json::json!({
                "path": "dup.txt",
                "edits": [{"oldText": "repeat", "newText": "changed"}]
            }), None)
            .await;

        assert!(result.is_err(), "Should error on duplicate oldText");
        let err = result.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("Found 2 occurrences"), "Got: {}", msg);
    }

    #[tokio::test]
    async fn edit_crlf_preserved() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        let file_path = Path::new(&dir).join("crlf.txt");
        tokio::fs::write(&file_path, "hello\r\nworld\r\n").await.unwrap();

        let result = tool
            .execute("c5", serde_json::json!({
                "path": "crlf.txt",
                "edits": [{"oldText": "world", "newText": "mars"}]
            }), None)
            .await
            .unwrap();

        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("Successfully"), "Got: {}", text);

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "hello\r\nmars\r\n", "CRLF should be preserved, got {:?}", content);
    }

    #[tokio::test]
    async fn edit_bom_preserved() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        let file_path = Path::new(&dir).join("bom.txt");
        // Write with UTF-8 BOM
        tokio::fs::write(&file_path, "\u{FEFF}hello world\n").await.unwrap();

        let result = tool
            .execute("c6", serde_json::json!({
                "path": "bom.txt",
                "edits": [{"oldText": "hello world", "newText": "goodbye"}]
            }), None)
            .await
            .unwrap();

        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("Successfully"), "Got: {}", text);

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert!(content.starts_with('\u{FEFF}'), "BOM should be preserved");
        assert!(content.contains("goodbye"), "Content should be updated");
    }

    #[tokio::test]
    async fn edit_fuzzy_match_smart_quotes() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        let file_path = Path::new(&dir).join("fuzzy.txt");
        // Use smart quotes in the file
        tokio::fs::write(&file_path, "it\u{2019}s fine\n").await.unwrap();

        // Model sends straight quotes
        let result = tool
            .execute("c7", serde_json::json!({
                "path": "fuzzy.txt",
                "edits": [{"oldText": "it's fine", "newText": "it's changed"}]
            }), None)
            .await
            .unwrap();

        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("Successfully"), "Fuzzy matching should handle smart quotes");
    }

    #[tokio::test]
    async fn edit_overlapping_edits_error() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        let file_path = Path::new(&dir).join("overlap.txt");
        tokio::fs::write(&file_path, "hello world fine\n").await.unwrap();

        let result = tool
            .execute("c8", serde_json::json!({
                "path": "overlap.txt",
                "edits": [
                    {"oldText": "hello world", "newText": "hi"},
                    {"oldText": "world fine", "newText": "earth"}
                ]
            }), None)
            .await;

        assert!(result.is_err(), "Should error on overlapping edits");
        let err = result.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("overlap"), "Got: {}", msg);
    }

    #[tokio::test]
    async fn edit_abort() {
        let (tool, tmp) = tool_and_temp().await;
        let _dir = tmp.lock().await.path().to_string_lossy().to_string();
        let (tx, rx) = tokio::sync::watch::channel(false);

        let handle = tokio::spawn(async move {
            tool
                .execute("c9", serde_json::json!({
                    "path": "any.txt",
                    "edits": [{"oldText": "x", "newText": "y"}]
                }), Some(rx))
                .await
        });

        tx.send(true).ok();

        let result = handle.await.unwrap().unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("aborted"), "Expected aborted, got: {}", text);
    }

    #[tokio::test]
    async fn edit_with_diff_output() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        let file_path = Path::new(&dir).join("diff_test.txt");
        tokio::fs::write(&file_path, "AAA\nBBB\nCCC\n").await.unwrap();

        let result = tool
            .execute("c10", serde_json::json!({
                "path": "diff_test.txt",
                "edits": [{"oldText": "BBB", "newText": "XXX"}]
            }), None)
            .await
            .unwrap();

        // Check that diff was generated in details
        let details = &result.details;
        assert!(details.get("diff").is_some(), "Should have diff");
        assert!(details.get("patch").is_some(), "Should have patch");
        let diff = details["diff"].as_str().unwrap();
        assert!(diff.contains("-2 BBB"), "Diff should show removed line, got: {}", diff);
        assert!(diff.contains("+2 XXX"), "Diff should show added line, got: {}", diff);
    }

    #[test]
    fn normalize_fuzzy_handles_smart_quotes() {
        let input = "hello\u{2018}world\u{201D}";
        let result = normalize_for_fuzzy_match(input);
        assert_eq!(result, "hello'world\"");
    }

    #[test]
    fn normalize_fuzzy_handles_dashes() {
        let input = "a\u{2013}b\u{2014}c";
        let result = normalize_for_fuzzy_match(input);
        assert_eq!(result, "a-b-c");
    }

    #[test]
    fn normalize_fuzzy_trims_trailing_whitespace() {
        let input = "hello   \nworld  \n";
        let result = normalize_for_fuzzy_match(input);
        assert_eq!(result, "hello\nworld\n");
    }

    #[test]
    fn strip_bom_removes_bom() {
        let (bom, text) = strip_bom("\u{FEFF}hello");
        assert_eq!(bom, "\u{FEFF}");
        assert_eq!(text, "hello");
    }

    #[test]
    fn strip_bom_no_bom() {
        let (bom, text) = strip_bom("hello");
        assert_eq!(bom, "");
        assert_eq!(text, "hello");
    }

    #[test]
    fn detect_line_ending_crlf() {
        assert_eq!(detect_line_ending("hello\r\nworld\r\n"), "\r\n");
    }

    #[test]
    fn detect_line_ending_lf() {
        assert_eq!(detect_line_ending("hello\nworld\n"), "\n");
    }

    #[test]
    fn normalize_to_lf_converts_crlf() {
        assert_eq!(normalize_to_lf("hello\r\nworld\r\n"), "hello\nworld\n");
    }

    #[test]
    fn restore_line_endings_to_crlf() {
        assert_eq!(restore_line_endings("hello\nworld\n", "\r\n"), "hello\r\nworld\r\n");
    }

    #[test]
    fn prepare_edit_arguments_legacy() {
        let params = serde_json::json!({
            "path": "test.txt",
            "oldText": "old",
            "newText": "new"
        });
        let result = prepare_edit_arguments(&params).unwrap();
        assert_eq!(result.path, "test.txt");
        assert_eq!(result.edits.len(), 1);
        assert_eq!(result.edits[0].old_text, "old");
        assert_eq!(result.edits[0].new_text, "new");
    }

    #[test]
    fn prepare_edit_arguments_edits_string() {
        let params = serde_json::json!({
            "path": "test.txt",
            "edits": "[{\"oldText\": \"a\", \"newText\": \"b\"}]"
        });
        let result = prepare_edit_arguments(&params).unwrap();
        assert_eq!(result.edits.len(), 1);
        assert_eq!(result.edits[0].old_text, "a");
    }
}
