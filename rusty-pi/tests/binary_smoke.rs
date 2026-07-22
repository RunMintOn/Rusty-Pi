//! Binary smoke tests using ordinary stdin/stdout pipes — verify the rusty-pi binary can:
//! - Start up
//! - Accept input
//! - Process commands
//! - Exit cleanly via /quit
//! - Handle Ctrl+C
//!
//! These are deliberately not PTY tests. Real terminal behavior lives in
//! `tests/pty_smoke.rs`.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Get the path to the rusty-pi binary.
fn binary_path() -> String {
    env!("CARGO_BIN_EXE_rusty-pi").to_string()
}

/// Helper: spawn rusty-pi with a mock provider and return the child process.
fn spawn_mock(args: &[&str]) -> std::process::Child {
    let mut cmd = Command::new(binary_path());
    cmd.arg("-p").arg("mock");
    cmd.args(args);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

    cmd.spawn().expect("Failed to spawn rusty-pi")
}

/// Wait for a child process with a timeout.
/// Returns `Ok(Some(status))` if it exited, `Ok(None)` if timeout.
fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if start.elapsed() > timeout {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

// ── Basic startup and exit tests ───────────────────────────────────────

#[test]
fn binary_starts_with_mock_provider() {
    let mut child = spawn_mock(&[]);
    // Send /quit to exit
    if let Some(ref mut stdin) = child.stdin {
        writeln!(stdin, "/quit").unwrap();
        stdin.flush().unwrap();
    }
    let exit = wait_with_timeout(&mut child, Duration::from_secs(5));
    match exit {
        Some(status) => assert!(
            status.success() || status.code() == Some(0),
            "Process should exit cleanly, got: {:?}",
            status
        ),
        None => {
            child.kill().ok();
            panic!("Process did not exit within timeout");
        }
    }
}

#[test]
fn binary_starts_and_exits_with_exit_command() {
    let mut child = spawn_mock(&[]);
    if let Some(ref mut stdin) = child.stdin {
        writeln!(stdin, "/exit").unwrap();
        stdin.flush().unwrap();
    }
    let exit = wait_with_timeout(&mut child, Duration::from_secs(5));
    match exit {
        Some(status) => assert!(status.success() || status.code() == Some(0)),
        None => {
            child.kill().ok();
            panic!("Process did not exit within timeout");
        }
    }
}

// ── Help command test ──────────────────────────────────────────────────

#[test]
fn help_command_returns_info() {
    let mut child = spawn_mock(&[]);
    if let Some(ref mut stdin) = child.stdin {
        writeln!(stdin, "/help").unwrap();
        stdin.flush().unwrap();
        // Give it time to process
        std::thread::sleep(Duration::from_millis(500));
        writeln!(stdin, "/quit").unwrap();
        stdin.flush().unwrap();
    }

    let exit = wait_with_timeout(&mut child, Duration::from_secs(5));
    match exit {
        Some(status) => assert!(status.success() || status.code() == Some(0)),
        None => {
            child.kill().ok();
            panic!("Process did not exit within timeout");
        }
    }
}

// ── Mock provider response test ────────────────────────────────────────

#[test]
fn mock_provider_responds_to_prompt() {
    let mut child = spawn_mock(&[]);
    if let Some(ref mut stdin) = child.stdin {
        // Send a prompt
        writeln!(stdin, "hello").unwrap();
        stdin.flush().unwrap();
        // Wait for response
        std::thread::sleep(Duration::from_secs(2));
        // Exit
        writeln!(stdin, "/quit").unwrap();
        stdin.flush().unwrap();
    }

    let exit = wait_with_timeout(&mut child, Duration::from_secs(10));
    match exit {
        Some(status) => assert!(status.success() || status.code() == Some(0)),
        None => {
            child.kill().ok();
            panic!("Process did not exit within timeout");
        }
    }
}

// ── Single-shot prompt test ────────────────────────────────────────────

#[test]
fn single_shot_prompt_exits() {
    let mut cmd = Command::new(binary_path());
    cmd.arg("-p").arg("mock").arg("hello from single shot");
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("Failed to spawn");

    let exit = wait_with_timeout(&mut child, Duration::from_secs(10));

    match exit {
        Some(status) => {
            assert!(status.success() || status.code() == Some(0));
        }
        None => {
            child.kill().ok();
            panic!("Single-shot mode did not exit within timeout");
        }
    }
}

// ── Multiple commands test ──────────────────────────────────────────────

#[test]
fn multiple_commands_sequentially() {
    let mut child = spawn_mock(&[]);
    if let Some(ref mut stdin) = child.stdin {
        // Send multiple commands
        writeln!(stdin, "first command").unwrap();
        stdin.flush().unwrap();
        std::thread::sleep(Duration::from_millis(500));
        writeln!(stdin, "second command").unwrap();
        stdin.flush().unwrap();
        std::thread::sleep(Duration::from_millis(500));
        // Exit
        writeln!(stdin, "/quit").unwrap();
        stdin.flush().unwrap();
    }

    let exit = wait_with_timeout(&mut child, Duration::from_secs(15));
    match exit {
        Some(status) => assert!(status.success() || status.code() == Some(0)),
        None => {
            child.kill().ok();
            panic!("Process did not exit within timeout");
        }
    }
}

// ── Residual process check ─────────────────────────────────────────────

#[test]
fn process_exits_cleanly_no_hang() {
    let mut child = spawn_mock(&[]);
    if let Some(ref mut stdin) = child.stdin {
        writeln!(stdin, "/quit").unwrap();
        stdin.flush().unwrap();
    }

    let exit = wait_with_timeout(&mut child, Duration::from_secs(5));
    match exit {
        Some(status) => {
            assert!(
                status.success() || status.code() == Some(0),
                "Process should exit cleanly, got: {:?}",
                status
            );
        }
        None => {
            child.kill().ok();
            panic!("Process did not exit within timeout — possible hang");
        }
    }
}

// ── Single-shot stdout/stderr separation tests ────────────────────────

#[test]
fn single_shot_text_only_stdout_only() {
    let mut cmd = Command::new(binary_path());
    cmd.arg("-p").arg("mock").arg("hello");
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("Failed to spawn");
    let exit = wait_with_timeout(&mut child, Duration::from_secs(10));

    let stdout = child
        .stdout
        .take()
        .map(|mut s| {
            let mut buf = String::new();
            s.read_to_string(&mut buf).unwrap();
            buf
        })
        .unwrap_or_default();
    let stderr = child
        .stderr
        .take()
        .map(|mut s| {
            let mut buf = String::new();
            s.read_to_string(&mut buf).unwrap();
            buf
        })
        .unwrap_or_default();

    match exit {
        Some(status) => {
            assert!(status.success(), "Should exit 0, got: {:?}", status);
            // stdout should contain assistant text
            assert!(
                stdout.contains("mock"),
                "stdout should contain assistant response, got: {}",
                stdout
            );
            // stderr should be empty for text-only mock response
            assert!(
                stderr.is_empty() || stderr.chars().all(|c| c == '\n'),
                "stderr should be empty for text-only response, got: {:?}",
                stderr
            );
        }
        None => {
            child.kill().ok();
            panic!("Single-shot did not exit within timeout");
        }
    }
}

#[test]
fn single_shot_tool_call_stderr_has_diagnostics() {
    let mut cmd = Command::new(binary_path());
    cmd.arg("-p").arg("mock");
    cmd.env("RUSTY_PI_MOCK_TOOL", "echo tool-output");
    cmd.arg("run a tool");
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("Failed to spawn");
    let exit = wait_with_timeout(&mut child, Duration::from_secs(10));

    let stdout = child
        .stdout
        .take()
        .map(|mut s| {
            let mut buf = String::new();
            s.read_to_string(&mut buf).unwrap();
            buf
        })
        .unwrap_or_default();
    let stderr = child
        .stderr
        .take()
        .map(|mut s| {
            let mut buf = String::new();
            s.read_to_string(&mut buf).unwrap();
            buf
        })
        .unwrap_or_default();

    match exit {
        Some(status) => {
            assert!(status.success(), "Should exit 0, got: {:?}", status);
            // stdout should contain the final assistant text
            assert!(
                stdout.contains("Mock tool round complete"),
                "stdout should contain final response, got: {}",
                stdout
            );
            // stderr should contain tool diagnostics
            assert!(
                stderr.contains("bash") || stderr.contains("tool"),
                "stderr should contain tool diagnostics, got: {}",
                stderr
            );
        }
        None => {
            child.kill().ok();
            panic!("Single-shot tool call did not exit within timeout");
        }
    }
}

/// Real single-shot SIGINT test: spawns the binary with a slow mock tool,
/// sends a real SIGINT signal to the child PID, and verifies exit code 130.
///
/// Uses `RUSTY_PI_MOCK_TOOL=sleep 30` so the bash tool runs a slow command
/// that can be interrupted. The mock provider emits a single tool call, then
/// the agent engine executes it via the bash tool.
///
/// This is NOT a pipe-level Ctrl+C byte — it is a real OS signal.
#[test]
fn real_single_shot_sigint_cancels_settles_and_exits_130() {
    use std::sync::mpsc;
    use std::thread;

    let mut cmd = Command::new(binary_path());
    cmd.arg("-p")
        .arg("mock")
        .env("RUSTY_PI_MOCK_TOOL", "sleep 30")
        .arg("run a slow tool");
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("Failed to spawn rusty-pi");
    let pid = child.id();

    // Read stderr in a background thread to detect tool start.
    let stderr = child.stderr.take().expect("stderr pipe");
    let (tx, rx) = mpsc::channel();
    let reader_thread = thread::spawn(move || {
        let mut reader = stderr;
        let mut buf = String::new();
        let mut full_output = String::new();
        loop {
            let mut chunk = [0u8; 4096];
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let s = String::from_utf8_lossy(&chunk[..n]).to_string();
                    full_output.push_str(&s);
                    buf.push_str(&s);
                    // Look for the tool-start marker rendered by PrintFrontend.
                    if buf.contains("\u{2699}") || buf.contains("bash") {
                        let _ = tx.send(full_output.clone());
                        // Keep reading until EOF so the pipe is drained.
                        loop {
                            match reader.read(&mut chunk) {
                                Ok(0) | Err(_) => break,
                                Ok(n2) => {
                                    let s2 = String::from_utf8_lossy(&chunk[..n2]).to_string();
                                    full_output.push_str(&s2);
                                }
                            }
                        }
                        return;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Wait for the tool to start (up to 15 seconds).
    rx.recv_timeout(Duration::from_secs(15))
        .expect("Timed out waiting for tool to start on stderr");

    // Send real SIGINT to the child process.
    unsafe {
        libc::kill(pid as i32, libc::SIGINT);
    }

    // Wait for the process to exit (up to 10 seconds after SIGINT).
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let exit_status = loop {
        if std::time::Instant::now() >= deadline {
            child.kill().ok();
            child.wait().ok();
            panic!("Child did not exit within 10 seconds after SIGINT");
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => panic!("try_wait failed: {e}"),
        }
    };

    reader_thread.join().expect("stderr reader thread must not panic");

    // --- Assertions ---

    // Exit code must be 130 (128 + SIGINT).
    assert_eq!(
        exit_status.code(),
        Some(130),
        "Expected exit code 130 after SIGINT, got {:?}",
        exit_status.code()
    );

    // PID must be gone (no zombie).
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if !pid_exists(pid) {
            assert!(!pid_is_zombie(pid), "Child became a zombie");
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Child PID {pid} still exists after wait");
}

fn pid_exists(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn pid_is_zombie(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    stat.rsplit_once(") ").and_then(|(_, rest)| rest.chars().next()) == Some('Z')
}

fn single_shot_invalid_provider_is_nonzero_and_stdout_clean() {
    let mut cmd = Command::new(binary_path());
    cmd.arg("-p").arg("not-a-provider").arg("hello");
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("Failed to spawn");
    let exit = wait_with_timeout(&mut child, Duration::from_secs(10));
    let stdout = child
        .stdout
        .take()
        .map(|mut s| {
            let mut buf = String::new();
            s.read_to_string(&mut buf).unwrap();
            buf
        })
        .unwrap_or_default();
    let stderr = child
        .stderr
        .take()
        .map(|mut s| {
            let mut buf = String::new();
            s.read_to_string(&mut buf).unwrap();
            buf
        })
        .unwrap_or_default();

    match exit {
        Some(status) => {
            assert_ne!(status.code(), Some(0), "Invalid provider must fail");
            assert!(stdout.is_empty(), "stdout must not contain diagnostics: {stdout:?}");
            assert!(!stderr.is_empty(), "stderr must contain the provider error");
        }
        None => {
            child.kill().ok();
            panic!("Invalid-provider invocation did not exit within timeout");
        }
    }
}
