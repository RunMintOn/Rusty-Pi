//! Real Unix PTY smoke tests.
//!
//! These tests intentionally use a pseudo-terminal master/slave pair. They do
//! not use stdin/stdout pipes: the child receives a controlling terminal, so
//! crossterm raw mode, alternate-screen setup, keyboard decoding, and
//! restoration all run through the real TTY path.

#![cfg(unix)]

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_rusty-pi")
}

struct PtyProcess {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output_rx: Receiver<Vec<u8>>,
    reader_thread: JoinHandle<()>,
    pid: u32,
}

impl PtyProcess {
    fn spawn(agent_dir: &std::path::Path) -> Self {
        Self::spawn_inner(agent_dir, None, false)
    }

    fn spawn_with_tool(agent_dir: &std::path::Path, mock_tool: Option<&str>) -> Self {
        Self::spawn_inner(agent_dir, mock_tool, false)
    }

    fn spawn_with_delayed_command(agent_dir: &std::path::Path) -> Self {
        Self::spawn_inner(agent_dir, None, true)
    }

    fn spawn_repl(agent_dir: &std::path::Path) -> Self {
        Self::spawn_repl_inner(agent_dir, None)
    }

    fn spawn_repl_with_tool(agent_dir: &std::path::Path, mock_tool: Option<&str>) -> Self {
        Self::spawn_repl_inner(agent_dir, mock_tool)
    }

    fn spawn_repl_inner(agent_dir: &std::path::Path, mock_tool: Option<&str>) -> Self {
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty must be available on Unix");

        let mut command = CommandBuilder::new(binary_path());
        // No --tui flag: runs the print-mode REPL.
        command.arg("-p");
        command.arg("mock");
        command.env("RUSTY_PI_AGENT_DIR", agent_dir);
        if let Some(mock_tool) = mock_tool {
            command.env("RUSTY_PI_MOCK_TOOL", mock_tool);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .expect("REPL child must start on the PTY slave");
        let pid = child.process_id().expect("PTY child must expose a PID");
        let reader = pair.master.try_clone_reader().expect("clone PTY reader");
        let writer = pair.master.take_writer().expect("take PTY writer");
        drop(pair.slave);

        let (output_tx, output_rx) = mpsc::channel();
        let reader_thread = thread::spawn(move || {
            let mut reader = reader;
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(size) => {
                        if output_tx.send(buffer[..size].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Self {
            child,
            writer,
            output_rx,
            reader_thread,
            pid,
        }
    }

    fn spawn_inner(agent_dir: &std::path::Path, mock_tool: Option<&str>, delayed_command: bool) -> Self {
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty must be available on Unix");

        let mut command = CommandBuilder::new(binary_path());
        command.arg("--tui");
        command.arg("-p");
        command.arg("mock");
        command.env("RUSTY_PI_AGENT_DIR", agent_dir);
        if let Some(mock_tool) = mock_tool {
            command.env("RUSTY_PI_MOCK_TOOL", mock_tool);
        }
        if delayed_command {
            command.env("RUSTY_PI_TUI_TEST_DELAYED_COMMAND", "1");
        }

        let child = pair
            .slave
            .spawn_command(command)
            .expect("TUI child must start on the PTY slave");
        let pid = child.process_id().expect("PTY child must expose a PID");
        let reader = pair.master.try_clone_reader().expect("clone PTY reader");
        let writer = pair.master.take_writer().expect("take PTY writer");
        drop(pair.slave);

        let (output_tx, output_rx) = mpsc::channel();
        let reader_thread = thread::spawn(move || {
            let mut reader = reader;
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(size) => {
                        if output_tx.send(buffer[..size].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Self {
            child,
            writer,
            output_rx,
            reader_thread,
            pid,
        }
    }

    fn send_bytes(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write keyboard input to PTY");
        self.writer.flush().expect("flush keyboard input to PTY");
    }

    fn wait_for(&self, needle: &str, timeout: Duration) -> Vec<u8> {
        let deadline = Instant::now() + timeout;
        let mut output = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.output_rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
                Ok(chunk) => {
                    output.extend_from_slice(&chunk);
                    if String::from_utf8_lossy(&output).contains(needle) {
                        return output;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        panic!(
            "timed out waiting for {needle:?}; PTY output was: {}",
            String::from_utf8_lossy(&output)
        );
    }

    fn finish(mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            match self.child.try_wait().expect("PTY try_wait must work") {
                Some(status) => break status,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                None => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!("PTY child did not exit within five seconds");
                }
            }
        };
        assert!(status.success(), "TUI child exited unsuccessfully: {status:?}");
        drop(self.writer);
        self.reader_thread.join().expect("PTY reader thread must join");

        // wait() above is the sole child reap operation. Verify that the PID
        // is gone rather than treating return from this helper as proof.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !process_exists(self.pid) {
                assert!(!process_is_zombie(self.pid), "PTY child became a zombie");
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("PTY child PID {} still exists after wait", self.pid);
    }
}

fn process_exists(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn process_is_zombie(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    stat.rsplit_once(") ").and_then(|(_, rest)| rest.chars().next()) == Some('Z')
}

#[test]
fn real_tui_pty() {
    let agent_dir = tempfile::tempdir().expect("temporary agent directory");
    let mut process = PtyProcess::spawn(agent_dir.path());

    // Seeing the rendered TUI frame proves crossterm successfully opened the
    // PTY as a terminal; a pipe cannot pass raw-mode/alternate-screen setup.
    process.wait_for("Transcript", Duration::from_secs(5));
    process.send_bytes(b"fixed prompt\r");
    // ANSI cursor/style sequences may occur between words in a rendered
    // frame; a stable response token is sufficient and avoids matching ANSI.
    process.wait_for("Hello", Duration::from_secs(10));

    // 0x03 is the terminal control character delivered by a real PTY in raw
    // mode, not an internal CancellationToken call.
    process.send_bytes(b"\x03");
    // Ctrl+C in an idle, empty editor is intentionally non-exiting. Use the
    // explicit command for process termination.
    process.send_bytes(b"/quit\r");
    process.finish();
}

#[test]
fn real_tui_pty_quit_command() {
    let agent_dir = tempfile::tempdir().expect("temporary agent directory");
    let mut process = PtyProcess::spawn(agent_dir.path());
    process.wait_for("Transcript", Duration::from_secs(5));
    process.send_bytes(b"/quit\r");
    process.finish();
}

#[test]
fn real_tui_pty_model_without_argument_never_enters_a_picker() {
    let agent_dir = tempfile::tempdir().expect("temporary agent directory");
    let mut process = PtyProcess::spawn(agent_dir.path());
    process.wait_for("Transcript", Duration::from_secs(5));
    process.send_bytes(b"/model\r");
    process.wait_for("Use: /model", Duration::from_secs(5));
    process.send_bytes(b"/quit\r");
    process.finish();
}

#[test]
fn real_tui_pty_context_without_argument_never_enters_a_picker() {
    let agent_dir = tempfile::tempdir().expect("temporary agent directory");
    let mut process = PtyProcess::spawn(agent_dir.path());
    process.wait_for("Transcript", Duration::from_secs(5));
    process.send_bytes(b"/context\r");
    process.wait_for("Use: /context", Duration::from_secs(5));
    process.send_bytes(b"/quit\r");
    process.finish();
}

#[test]
fn real_tui_pty_context_argument_is_async_and_restores_terminal() {
    let agent_dir = tempfile::tempdir().expect("temporary agent directory");
    let context_dir = tempfile::tempdir().expect("temporary context directory");
    let context_path = context_dir.path().join("context file.md");
    std::fs::write(&context_path, "pty context").expect("write context file");
    let mut process = PtyProcess::spawn(agent_dir.path());
    process.wait_for("Transcript", Duration::from_secs(5));
    let command = format!("/context {}\r", context_path.display());
    process.send_bytes(command.as_bytes());
    process.wait_for("Added", Duration::from_secs(5));
    process.send_bytes(b"/quit\r");
    process.finish();
}

#[test]
fn real_tui_pty_multiline_tool_output_scroll_and_end() {
    let agent_dir = tempfile::tempdir().expect("temporary agent directory");
    let mut process = PtyProcess::spawn_with_tool(
        agent_dir.path(),
        Some("for i in $(seq 1 40); do printf 'out-%s\\n' $i; printf 'err-%s\\n' $i >&2; done"),
    );
    process.wait_for("Transcript", Duration::from_secs(5));

    // Ctrl+J is the portable multiline binding used by the reducer.
    process.send_bytes(b"first line\x0asecond line\r");
    process.wait_for("Mock tool round complete", Duration::from_secs(10));

    // Tab focuses the transcript, Space expands the selected tool, and
    // PageUp enters browsing mode. The next prompt must not pull the view
    // back to the bottom; the title exposes the unread count.
    process.send_bytes(b"\t ");
    process.send_bytes(b"\x1b[5~");
    process.wait_for("err-", Duration::from_secs(2));
    // Home is the explicit beginning-of-transcript operation, where the
    // stdout half of the expanded tool is visible.
    process.send_bytes(b"\x1b[H");
    process.wait_for("out-", Duration::from_secs(2));
    process.send_bytes(b"i");
    process.send_bytes(b"next prompt\r");
    process.wait_for("new lines", Duration::from_secs(5));

    // End returns to follow mode. Explicit /quit is used because Ctrl+C in an
    // idle empty editor intentionally does not terminate the application.
    process.send_bytes(b"\x1b[F");
    process.send_bytes(b"/quit\r");
    process.finish();
}

#[test]
fn real_tui_pty_ctrl_c_aborts_running_tool() {
    let agent_dir = tempfile::tempdir().expect("temporary agent directory");
    let mut process = PtyProcess::spawn_with_tool(agent_dir.path(), Some("sleep 3; echo should-not-finish"));
    process.wait_for("Transcript", Duration::from_secs(5));
    process.send_bytes(b"run a slow tool\r");
    process.wait_for("running", Duration::from_secs(5));
    process.send_bytes(b"\x03");
    process.wait_for("Aborted", Duration::from_secs(5));
    process.send_bytes(b"/quit\r");
    process.finish();
}

#[test]
fn real_tui_pty_ctrl_c_cancels_delayed_command_and_accepts_next_input() {
    let agent_dir = tempfile::tempdir().expect("temporary agent directory");
    let mut process = PtyProcess::spawn_with_delayed_command(agent_dir.path());
    process.wait_for("Transcript", Duration::from_secs(5));

    process.send_bytes(b"/test-delayed\r");
    process.wait_for("Command running", Duration::from_secs(5));

    // Navigation is delivered through the real PTY before the cancellation.
    process.send_bytes(b"\x1b[5~");
    // 0x03 is the real PTY Ctrl+C byte, not a direct token cancellation.
    process.send_bytes(b"\x03");
    process.wait_for("Command cancelled", Duration::from_secs(5));

    process.send_bytes(b"/help\r");
    process.wait_for("Commands:", Duration::from_secs(5));
    process.send_bytes(b"/quit\r");
    process.finish();
}

// ── Non-TUI REPL PTY tests ───────────────────────────────────────────────

/// Real non-TUI REPL PTY SIGINT test.
///
/// Starts rusty-pi in print-mode REPL (no `--tui`), sends a slow task,
/// interrupts it with a real PTY Ctrl+C, verifies the REPL continues,
/// then confirms a second prompt succeeds and /quit exits cleanly.
///
/// This test must NOT be replaced by any TUI PTY test.
#[test]
fn real_repl_pty_sigint_cancels_run_and_accepts_next_input() {
    let agent_dir = tempfile::tempdir().expect("temporary agent directory");
    let mut process = PtyProcess::spawn_repl_with_tool(agent_dir.path(), Some("sleep 30; echo should-not-finish"));

    // Wait for the REPL banner and prompt.
    process.wait_for("> ", Duration::from_secs(5));

    // Submit a slow task.
    process.send_bytes(b"run a slow tool\r");

    // Wait for the tool to start (the gear icon + bash on stderr).
    process.wait_for("\u{2699}", Duration::from_secs(10));

    // Send real PTY Ctrl+C (0x03) to cancel the running agent run.
    process.send_bytes(b"\x03");

    // The driver should emit "Aborted" or "Run aborted".
    process.wait_for("bort", Duration::from_secs(5));

    // The REPL must still be alive — wait for the prompt.
    process.wait_for("> ", Duration::from_secs(5));

    // Submit a quick prompt to confirm the REPL is fully operational.
    process.send_bytes(b"/help\r");
    process.wait_for("Commands:", Duration::from_secs(5));

    // Clean exit.
    process.send_bytes(b"/quit\r");
    process.finish();
}
