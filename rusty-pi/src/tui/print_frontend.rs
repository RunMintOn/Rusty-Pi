//! PrintFrontend — consumes AgentEvent and writes to terminal.
//!
//! This is the bare-terminal frontend that replaces direct println!/eprintln!
//! calls in the core layer. All terminal output goes through this module.

use crate::agent::events::{AgentEvent, ToolOutputStream};
use crate::agent::types::AgentToolResult;
use crate::ai::types::StopReason;
use std::io::{self, Write};

/// A frontend that prints agent events to stdout/stderr.
pub struct PrintFrontend {
    /// Whether to show tool details (arguments, results).
    verbose: bool,
}

impl PrintFrontend {
    /// Create a new PrintFrontend with default settings.
    pub fn new() -> Self {
        Self { verbose: true }
    }

    /// Create a PrintFrontend with custom verbosity.
    pub fn with_verbose(verbose: bool) -> Self {
        Self { verbose }
    }

    /// Process a single agent event, writing appropriate output to the terminal.
    pub fn handle_event(&self, event: &AgentEvent) {
        match event {
            AgentEvent::RunStarted => {
                // No output needed for run start
            }
            AgentEvent::TextDelta { text } => {
                print!("{}", text);
                let _ = io::stdout().flush();
            }
            AgentEvent::ThinkingDelta { text } => {
                // Thinking content goes to stderr (not mixed with response)
                eprint!("[thinking] {}", text);
                let _ = io::stderr().flush();
            }
            AgentEvent::ToolStarted { id: _, name, arguments } => {
                if self.verbose {
                    let args_str = if arguments.is_object() && arguments.as_object().unwrap().is_empty() {
                        String::new()
                    } else {
                        format!(" {}", arguments)
                    };
                    print!("\n⚙ {}{}\n", name, args_str);
                } else {
                    print!("\n⚙ {}...\n", name);
                }
                let _ = io::stdout().flush();
            }
            AgentEvent::ToolOutput { id: _, stream, chunk } => {
                match stream {
                    ToolOutputStream::Stdout => {
                        print!("{}", chunk);
                    }
                    ToolOutputStream::Stderr => {
                        eprint!("{}", chunk);
                    }
                }
                let _ = io::stdout().flush();
                let _ = io::stderr().flush();
            }
            AgentEvent::ToolFinished { id: _, name: _, result } => {
                self.print_tool_result(result);
            }
            AgentEvent::ProviderError { error } => {
                eprintln!("\n❌ Provider error: {}", error.message);
                let _ = io::stderr().flush();
            }
            AgentEvent::RunAborted => {
                eprintln!("\n⏹ Run aborted");
                let _ = io::stderr().flush();
            }
            AgentEvent::RunFinished { stop_reason } => {
                match stop_reason {
                    StopReason::Stop => {
                        // Normal completion, no extra output
                    }
                    StopReason::Length => {
                        eprintln!("\n⚠ Response truncated (length limit)");
                    }
                    StopReason::Error => {
                        // Error already printed via ProviderError
                    }
                    StopReason::Aborted => {
                        // Already handled by RunAborted
                    }
                    StopReason::ToolUse => {
                        // Should not happen as a final stop reason
                    }
                }
                let _ = io::stderr().flush();
            }
        }
    }

    /// Print tool result details.
    fn print_tool_result(&self, result: &AgentToolResult) {
        if !self.verbose {
            return;
        }

        if result.is_error {
            // Extract error text from content
            let error_text = result
                .content
                .iter()
                .filter_map(|c| match c {
                    crate::ai::types::Content::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .next()
                .unwrap_or("unknown error");

            if let Some(code) = result.exit_code {
                eprintln!("  ❌ exit code {} — {}", code, error_text);
            } else if result.timed_out {
                eprintln!("  ⏰ timed out — {}", error_text);
            } else if result.aborted {
                eprintln!("  ⏹ aborted — {}", error_text);
            } else {
                eprintln!("  ❌ error — {}", error_text);
            }
            let _ = io::stderr().flush();
        } else {
            // Show first line of output for successful tools
            let output_line = result
                .content
                .iter()
                .filter_map(|c| match c {
                    crate::ai::types::Content::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .next()
                .unwrap_or("");

            let first_line = output_line.lines().next().unwrap_or("");
            if !first_line.is_empty() {
                let display = if first_line.len() > 80 {
                    format!("{}...", &first_line[..77])
                } else {
                    first_line.to_string()
                };
                println!("  ✅ {}", display);
                let _ = io::stdout().flush();
            }
        }
    }

    /// Drain all events from a receiver, printing each one.
    pub async fn run(&self, mut rx: tokio::sync::mpsc::Receiver<AgentEvent>) {
        while let Some(event) = rx.recv().await {
            self.handle_event(&event);
        }
    }
}

impl Default for PrintFrontend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::events::ProviderError;
    use crate::ai::types::Content;

    #[test]
    fn print_frontend_handles_text_delta() {
        let frontend = PrintFrontend::new();
        // Just verify it doesn't panic
        frontend.handle_event(&AgentEvent::TextDelta { text: "hello".into() });
    }

    #[test]
    fn print_frontend_handles_tool_started() {
        let frontend = PrintFrontend::new();
        frontend.handle_event(&AgentEvent::ToolStarted {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        });
    }

    #[test]
    fn print_frontend_handles_tool_finished_ok() {
        let frontend = PrintFrontend::new();
        frontend.handle_event(&AgentEvent::ToolFinished {
            id: "tc_1".into(),
            name: "bash".into(),
            result: AgentToolResult {
                content: vec![Content::Text {
                    text: "file.txt".into(),
                }],
                ..Default::default()
            },
        });
    }

    #[test]
    fn print_frontend_handles_tool_finished_error() {
        let frontend = PrintFrontend::new();
        frontend.handle_event(&AgentEvent::ToolFinished {
            id: "tc_1".into(),
            name: "bash".into(),
            result: AgentToolResult {
                content: vec![Content::Text {
                    text: "command not found".into(),
                }],
                is_error: true,
                exit_code: Some(127),
                ..Default::default()
            },
        });
    }

    #[test]
    fn print_frontend_handles_provider_error() {
        let frontend = PrintFrontend::new();
        frontend.handle_event(&AgentEvent::ProviderError {
            error: ProviderError {
                reason: StopReason::Error,
                message: "API limit exceeded".into(),
            },
        });
    }

    #[test]
    fn print_frontend_handles_run_aborted() {
        let frontend = PrintFrontend::new();
        frontend.handle_event(&AgentEvent::RunAborted);
    }

    #[test]
    fn print_frontend_handles_run_finished() {
        let frontend = PrintFrontend::new();
        frontend.handle_event(&AgentEvent::RunFinished {
            stop_reason: StopReason::Stop,
        });
    }

    #[test]
    fn print_frontend_non_verbose_hides_details() {
        let frontend = PrintFrontend::with_verbose(false);
        frontend.handle_event(&AgentEvent::ToolStarted {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        });
        frontend.handle_event(&AgentEvent::ToolFinished {
            id: "tc_1".into(),
            name: "bash".into(),
            result: AgentToolResult {
                content: vec![Content::Text {
                    text: "file.txt".into(),
                }],
                ..Default::default()
            },
        });
        // Should not panic even in non-verbose mode
    }
}
