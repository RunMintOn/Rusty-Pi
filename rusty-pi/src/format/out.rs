//! Output formatting — structured terminal output via sparcli.
//!
//! All formatting functions return `String` so they can be tested by asserting
//! on the returned text without writing to a real terminal.

use sparcli::{Alert, Badge, KeyValue, Renderable, Table};

/// Summary of session state for display purposes.
///
/// Filled by [`Session::get_info()`] (ticket 22) and consumed by
/// [`OutputFormatter::session_info()`].
pub struct SessionInfo {
    pub id: String,
    pub model: String,
    pub msg_count: usize,
    pub cwd: String,
}

/// Summary of a stored session for listing purposes.
pub struct SessionSummary {
    pub id: String,
    pub model: String,
    pub msg_count: usize,
    pub created: String,
}

/// Central formatter for all user-visible terminal output.
///
/// Uses [`sparcli`] components internally. Every public method returns a
/// `String` — never writes directly to stdout — so callers decide where to
/// send the output and tests can assert on the content.
pub struct OutputFormatter;

impl OutputFormatter {
    /// Create a new formatter.
    pub fn new() -> Self {
        Self
    }

    /// Format session information as a key-value list.
    ///
    /// Example output:
    /// ```text
    /// Session:    abc123
    /// Model:      deepseek-v4-flash
    /// Messages:   5
    /// CWD:        /project
    /// ```
    pub fn session_info(&self, info: &SessionInfo) -> String {
        let mut buf = Vec::new();
        KeyValue::new()
            .add("Session", info.id.as_str())
            .add("Model", info.model.as_str())
            .add("Messages", info.msg_count.to_string())
            .add("CWD", info.cwd.as_str())
            .print_to(&mut buf)
            .expect("write to in-memory buffer");
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Format a session list as a table (for `/list-sessions`).
    ///
    /// Each summary should have columns ordered as: session id, model, messages, created.
    pub fn session_list(&self, sessions: &[SessionSummary]) -> String {
        if sessions.is_empty() {
            return "No sessions found.".to_string();
        }
        let mut buf = Vec::new();
        let mut table = Table::new()
            .columns(["Session", "Model", "Messages", "Created"]);
        for s in sessions {
            table = table.row([
                s.id.as_str(),
                s.model.as_str(),
                &s.msg_count.to_string(),
                s.created.as_str(),
            ]);
        }
        table
            .striped(true)
            .print_to(&mut buf)
            .expect("write to in-memory buffer");
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Format a tool start line (badge + args).
    pub fn tool_start(&self, name: &str, args: &str) -> String {
        let mut buf = Vec::new();
        Badge::new(name)
            .print_to(&mut buf)
            .expect("write to in-memory buffer");
        use std::io::Write;
        let _ = write!(buf, " {}", args);
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Format a tool end line (tool name + separator + duration).
    pub fn tool_end(&self, name: &str, duration_ms: u64) -> String {
        format!(
            "─── {}: {}.{:01}s ───\n",
            name,
            duration_ms / 1000,
            (duration_ms % 1000) / 100
        )
    }

    /// Format a tool error as a red alert.
    pub fn tool_error(&self, tool: &str, error: &str) -> String {
        let mut buf = Vec::new();
        Alert::error(format!("[{}] {}", tool, error))
            .print_to(&mut buf)
            .expect("write to in-memory buffer");
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Format a startup banner showing provider, model, session id.
    pub fn banner(&self, provider: &str, model: &str, session_id: &str) -> String {
        format!(
            "rusty-pi | {} | {} | {}",
            Badge::new(provider).render(80).plain().trim(),
            Badge::new(model).render(80).plain().trim(),
            session_id,
        )
    }

    /// Format an interrupt/cancellation message.
    pub fn interrupt(&self) -> String {
        let mut buf = Vec::new();
        Alert::warning("Aborted by user")
            .print_to(&mut buf)
            .expect("write to in-memory buffer");
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Format an error message as a red alert box.
    pub fn error(&self, msg: &str) -> String {
        let mut buf = Vec::new();
        Alert::error(msg)
            .print_to(&mut buf)
            .expect("write to in-memory buffer");
        String::from_utf8_lossy(&buf).to_string()
    }
}

impl Default for OutputFormatter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_info_contains_id() {
        let fmt = OutputFormatter::new();
        let info = SessionInfo {
            id: "test-123".into(),
            model: "deepseek-v4-flash".into(),
            msg_count: 3,
            cwd: "/project".into(),
        };
        let out = fmt.session_info(&info);
        assert!(out.contains("test-123"), "output should contain session id");
        assert!(out.contains("deepseek-v4-flash"), "output should contain model");
        assert!(out.contains("3"), "output should contain message count");
        assert!(out.contains("/project"), "output should contain cwd");
    }

    #[test]
    fn test_session_info_has_key_labels() {
        let fmt = OutputFormatter::new();
        let info = SessionInfo {
            id: "sess-1".into(),
            model: "mock".into(),
            msg_count: 0,
            cwd: "/".into(),
        };
        let out = fmt.session_info(&info);
        // KeyValue aligns keys as labels
        assert!(out.contains("Session"), "should have Session label");
        assert!(out.contains("Model"), "should have Model label");
        assert!(out.contains("Messages"), "should have Messages label");
        assert!(out.contains("CWD"), "should have CWD label");
    }

    #[test]
    fn test_error_output_contains_message() {
        let fmt = OutputFormatter::new();
        let out = fmt.error("something broke");
        assert!(out.contains("something broke"), "error output should contain the message");
    }

    #[test]
    fn test_error_output_is_alert_styled() {
        let fmt = OutputFormatter::new();
        let out = fmt.error("fail");
        // Alert::error produces box-drawing characters
        assert!(out.contains('╭'), "error alert should have top-left corner");
        assert!(out.contains('╭'), "error alert should have top-left corner");
    }

    #[test]
    fn test_session_info_empty_fields() {
        let fmt = OutputFormatter::new();
        let info = SessionInfo {
            id: "".into(),
            model: "".into(),
            msg_count: 0,
            cwd: "".into(),
        };
        let out = fmt.session_info(&info);
        // Should still produce valid output without panicking
        assert!(!out.is_empty(), "should produce output even with empty fields");
    }

    #[test]
    fn test_tool_start_contains_name() {
        let fmt = OutputFormatter::new();
        let out = fmt.tool_start("bash", "cargo test");
        assert!(out.contains("bash"), "tool_start should contain tool name");
        assert!(out.contains("cargo test"), "tool_start should contain args");
    }

    #[test]
    fn test_tool_end_contains_duration() {
        let fmt = OutputFormatter::new();
        let out = fmt.tool_end("bash", 1500);
        assert!(out.contains("1.5s"), "tool_end should contain formatted duration");
        assert!(out.contains("───"), "tool_end should have separator");
        assert!(out.contains("bash"), "tool_end should contain tool name");
    }

    #[test]
    fn test_tool_error_contains_tool_and_message() {
        let fmt = OutputFormatter::new();
        let out = fmt.tool_error("edit", "oldText not found");
        assert!(out.contains("edit"), "tool_error should contain tool name");
        assert!(out.contains("oldText not found"), "tool_error should contain error message");
    }

    #[test]
    fn test_tool_end_zero_duration() {
        let fmt = OutputFormatter::new();
        let out = fmt.tool_end("read", 0);
        assert!(out.contains("0.0s"));
        assert!(out.contains("read"));
    }

    #[test]
    fn test_error_empty_message() {
        let fmt = OutputFormatter::new();
        let out = fmt.error("");
        assert!(!out.is_empty(), "should produce output even with empty message");
    }

    #[test]
    fn test_session_list_empty() {
        let fmt = OutputFormatter::new();
        let out = fmt.session_list(&[]);
        assert!(out.contains("No sessions found"));
    }

    #[test]
    fn test_session_list_contains_entries() {
        let fmt = OutputFormatter::new();
        let sessions = vec![
            SessionSummary {
                id: "sess-1".into(),
                model: "deepseek-v4-flash".into(),
                msg_count: 5,
                created: "2m ago".into(),
            },
        ];
        let out = fmt.session_list(&sessions);
        assert!(out.contains("sess-1"), "should contain session id");
        assert!(out.contains("deepseek-v4-flash"), "should contain model");
        assert!(out.contains("5"), "should contain message count");
        assert!(out.contains("Session"), "should have column header");
    }

    #[test]
    fn test_output_formatter_default() {
        let fmt = OutputFormatter::default();
        let out = fmt.error("default test");
        assert!(out.contains("default test"));
    }
}
