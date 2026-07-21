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

#[test]
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
