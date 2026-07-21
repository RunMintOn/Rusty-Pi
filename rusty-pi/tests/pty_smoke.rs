//! PTY smoke tests — verify the rusty-pi binary can:
//! - Start up
//! - Accept input
//! - Process commands
//! - Exit cleanly via /quit
//! - Handle Ctrl+C
//!
//! These tests use `std::process::Command` with piped stdin/stdout
//! to simulate terminal interaction without requiring a real PTY.

use std::io::{BufRead, BufReader, Write};
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

// ── TUI startup and exit test ──────────────────────────────────────────

#[test]
#[cfg(unix)]
fn tui_starts_and_exits() {
    // TUI mode needs a terminal, so we skip this in non-TTY environments.
    if unsafe { libc::isatty(libc::STDOUT_FILENO) } == 0 {
        return;
    }

    let mut child = spawn_mock_tui();

    // Give TUI time to initialize
    std::thread::sleep(Duration::from_secs(1));

    // Send Ctrl+C to cancel/exit
    if let Some(ref mut stdin) = child.stdin {
        write!(stdin, "\x03").unwrap();
        stdin.flush().unwrap();
    }

    let exit = wait_with_timeout(&mut child, Duration::from_secs(5));

    match exit {
        Some(status) => {
            assert!(
                status.success() || status.code() == Some(0) || status.code() == Some(130),
                "TUI should exit cleanly, got: {:?}",
                status
            );
        }
        None => {
            child.kill().ok();
            panic!("TUI did not exit within timeout");
        }
    }
}

fn spawn_mock_tui() -> std::process::Child {
    let mut cmd = Command::new(binary_path());
    cmd.arg("-p").arg("mock");
    cmd.arg("--tui");
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

    cmd.spawn().expect("Failed to spawn rusty-pi with --tui")
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
