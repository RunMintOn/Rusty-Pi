//! Shared output truncation utilities for tool results.
//!
//! Mirrors the original `@earendil-works/pi-coding-agent/src/core/tools/truncate.ts`.
//!
//! Truncation is based on two independent limits — whichever is hit first wins:
//! - Line limit (default: 2000 lines)
//! - Byte limit (default: 50 KB)
//!
//! `truncate_head` keeps the **first** N lines/bytes (for file reads).
//! `truncate_tail` keeps the **last** N lines/bytes (for bash output).
//! Both never return partial lines (except a tail edge case).

/// Default maximum number of output lines before truncation.
pub const DEFAULT_MAX_LINES: usize = 2000;

/// Default maximum output bytes (50 KB) before truncation.
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;

/// Result of a truncation operation.
#[derive(Debug, Clone)]
pub struct TruncationResult {
    /// The truncated content.
    pub content: String,
    /// Whether truncation occurred.
    pub truncated: bool,
    /// Which limit was hit: "lines", "bytes", or empty string if not truncated.
    pub truncated_by: &'static str,
    /// Total number of lines in the original content.
    pub total_lines: usize,
    /// Total number of bytes in the original content.
    pub total_bytes: usize,
    /// Number of complete lines in the truncated output.
    pub output_lines: usize,
    /// Number of bytes in the truncated output.
    pub output_bytes: usize,
    /// Whether the first line exceeded the byte limit (for head truncation).
    pub first_line_exceeds_limit: bool,
    /// The max lines limit that was applied.
    pub max_lines: usize,
    /// The max bytes limit that was applied.
    pub max_bytes: usize,
}

/// Format bytes as human-readable size string.
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Split content into lines, stripping a trailing newline so the last empty
/// "line" from a trailing `\n` is not counted.
fn split_lines(content: &str) -> Vec<&str> {
    let trimmed = content.strip_suffix('\n').unwrap_or(content);
    if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('\n').collect()
    }
}

/// Truncate content from the **head** (keep first N lines/bytes).
///
/// Suitable for file reads where you want to see the beginning.
/// Never returns partial lines. If first line exceeds byte limit,
/// returns empty content with `first_line_exceeds_limit = true`.
pub fn truncate_head(content: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let total_bytes = content.len();
    let lines = split_lines(content);
    let total_lines = lines.len();

    // Check if no truncation needed
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: "",
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
            first_line_exceeds_limit: false,
            max_lines,
            max_bytes,
        };
    }

    // Check if first line alone exceeds byte limit
    if !lines.is_empty() {
        let first_line_bytes = lines[0].len();
        if first_line_bytes > max_bytes {
            return TruncationResult {
                content: String::new(),
                truncated: true,
                truncated_by: "bytes",
                total_lines,
                total_bytes,
                output_lines: 0,
                output_bytes: 0,
                first_line_exceeds_limit: true,
                max_lines,
                max_bytes,
            };
        }
    }

    // Collect complete lines from the start that fit
    let mut output_lines_vec: Vec<&str> = Vec::new();
    let mut output_bytes_count: usize = 0;
    let mut truncated_by: &'static str = "lines";

    for line in &lines {
        // +1 for newline except for the first line
        let line_bytes = line.len() + if output_lines_vec.is_empty() { 0 } else { 1 };

        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = "bytes";
            break;
        }

        if output_lines_vec.len() >= max_lines {
            truncated_by = "lines";
            break;
        }

        output_lines_vec.push(line);
        output_bytes_count += line_bytes;
    }

    let output_content = output_lines_vec.join("\n");
    let final_output_bytes = output_content.len();

    TruncationResult {
        content: output_content,
        truncated: true,
        truncated_by,
        total_lines,
        total_bytes,
        output_lines: output_lines_vec.len(),
        output_bytes: final_output_bytes,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

/// Truncate content from the **tail** (keep last N lines/bytes).
///
/// Suitable for bash output where you want to see the end (errors, final results).
/// Always keeps complete lines.
pub fn truncate_tail(content: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let total_bytes = content.len();
    let lines = split_lines(content);
    let total_lines = lines.len();

    // Check if no truncation needed
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: "",
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
            first_line_exceeds_limit: false,
            max_lines,
            max_bytes,
        };
    }

    // Work backwards, collecting complete lines from the end
    let mut output_lines_vec: Vec<&str> = Vec::new();
    let mut output_bytes_count: usize = 0;
    let mut truncated_by: &'static str = "lines";

    for line in lines.iter().rev() {
        // +1 for newline except for the first added line
        let line_bytes = line.len() + if output_lines_vec.is_empty() { 0 } else { 1 };

        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = "bytes";
            break;
        }

        if output_lines_vec.len() >= max_lines {
            truncated_by = "lines";
            break;
        }

        output_lines_vec.push(line);
        output_bytes_count += line_bytes;
    }

    output_lines_vec.reverse();
    let output_content = output_lines_vec.join("\n");
    let final_output_bytes = output_content.len();

    TruncationResult {
        content: output_content,
        truncated: true,
        truncated_by,
        total_lines,
        total_bytes,
        output_lines: output_lines_vec.len(),
        output_bytes: final_output_bytes,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- truncate_head tests ---

    #[test]
    fn head_no_truncation_needed() {
        let result = truncate_head("hello\nworld", 100, 99999);
        assert!(!result.truncated);
        assert_eq!(result.content, "hello\nworld");
        assert_eq!(result.total_lines, 2);
        assert_eq!(result.output_lines, 2);
    }

    #[test]
    fn head_truncated_by_lines() {
        let input = (1..=100).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let result = truncate_head(&input, 10, 99999);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, "lines");
        assert_eq!(result.output_lines, 10);
        assert_eq!(result.total_lines, 100);
        // Should keep lines 1-10 (first 10 of 100)
        assert!(result.content.starts_with("1"), "content: {:?}", result.content);
        assert!(result.content.ends_with("10"), "content: {:?}", result.content);
    }

    #[test]
    fn head_truncated_by_bytes() {
        let input = "short\n".to_owned() + &"a".repeat(100);
        let result = truncate_head(&input, 9999, 20);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, "bytes");
        // First line "short" should be kept (~6 bytes), second line long is cut
        assert_eq!(result.content, "short");
    }

    #[test]
    fn head_first_line_exceeds_limit() {
        let long_line = "x".repeat(5000);
        let result = truncate_head(&long_line, 9999, 100);
        assert!(result.truncated);
        assert!(result.first_line_exceeds_limit);
        assert_eq!(result.content, "");
        assert_eq!(result.output_lines, 0);
    }

    #[test]
    fn head_empty_content() {
        let result = truncate_head("", 100, 99999);
        assert!(!result.truncated);
        assert_eq!(result.content, "");
        assert_eq!(result.total_lines, 0);
    }

    #[test]
    fn head_exact_line_limit() {
        let input = (1..=10).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let result = truncate_head(&input, 10, 99999);
        assert!(!result.truncated, "Should NOT truncate when exactly at limit");
        assert_eq!(result.content, input);
    }

    // --- truncate_tail tests ---

    #[test]
    fn tail_no_truncation_needed() {
        let result = truncate_tail("hello\nworld", 100, 99999);
        assert!(!result.truncated);
        assert_eq!(result.content, "hello\nworld");
    }

    #[test]
    fn tail_truncated_by_lines() {
        let input = (1..=100).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let result = truncate_tail(&input, 10, 99999);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, "lines");
        assert_eq!(result.output_lines, 10);
        assert_eq!(result.total_lines, 100);
        // Should keep lines 91-100 (last 10 of 100)
        assert!(result.content.starts_with("91"), "content: {:?}", result.content);
        assert!(result.content.ends_with("100"), "content: {:?}", result.content);
    }

    #[test]
    fn tail_truncated_by_bytes() {
        let input = "short\n".to_owned() + &"a".repeat(5000);
        let result = truncate_tail(&input, 9999, 100);
        assert!(result.truncated);
        assert_eq!(result.truncated_by, "bytes");
        // The long line should be truncated; "short" may not fit either
        assert!(result.content.len() <= 100 + 1, "output too large: {}", result.content.len());
    }

    #[test]
    fn tail_empty_content() {
        let result = truncate_tail("", 100, 99999);
        assert!(!result.truncated);
        assert_eq!(result.content, "");
        assert_eq!(result.total_lines, 0);
    }
}
