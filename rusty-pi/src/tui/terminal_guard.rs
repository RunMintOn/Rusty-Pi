//! TerminalGuard — RAII guard for terminal state restoration.
//!
//! Ensures that raw mode, alternate screen, and cursor visibility are
//! restored when the guard is dropped, even on panic or error.

use crossterm::execute;
use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// RAII guard that restores terminal state on drop.
///
/// On creation, enables raw mode and alternate screen.
/// On drop, disables raw mode, leaves alternate screen, and shows cursor.
/// Any restoration failures are logged to stderr but do not panic.
pub struct TerminalGuard {
    stdout: Stdout,
    /// Track whether we've already cleaned up (to avoid double-restore).
    cleaned: bool,
}

/// Global flag to detect if a previous guard didn't clean up properly.
static GUARD_ACTIVE: AtomicBool = AtomicBool::new(false);

impl TerminalGuard {
    /// Create a new terminal guard, entering raw mode and alternate screen.
    ///
    /// Returns `Err` if terminal initialization fails.
    pub fn new() -> io::Result<Self> {
        // Check for leaked guard from previous run
        if GUARD_ACTIVE.swap(true, Ordering::SeqCst) {
            eprintln!("[TerminalGuard] WARNING: previous guard was not dropped properly");
        }

        let mut stdout = io::stdout();
        crossterm::terminal::enable_raw_mode()?;
        execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
        execute!(stdout, crossterm::cursor::Hide)?;

        Ok(Self { stdout, cleaned: false })
    }

    /// Perform the restoration steps.
    fn restore(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;
        GUARD_ACTIVE.store(false, Ordering::SeqCst);

        // Best-effort restoration — log errors but don't panic
        let _ = crossterm::terminal::disable_raw_mode();
        if let Err(e) = execute!(self.stdout, crossterm::terminal::LeaveAlternateScreen) {
            eprintln!("[TerminalGuard] failed to leave alternate screen: {}", e);
        }
        if let Err(e) = execute!(self.stdout, crossterm::cursor::Show) {
            eprintln!("[TerminalGuard] failed to show cursor: {}", e);
        }
        let _ = self.stdout.flush();
    }

    /// Get a reference to the underlying stdout for rendering.
    pub fn stdout(&mut self) -> &mut Stdout {
        &mut self.stdout
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Check if a terminal guard is currently active (for testing).
pub fn guard_is_active() -> bool {
    GUARD_ACTIVE.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests require a real terminal (TTY). They are ignored in CI
    // and headless environments. Run with: cargo test -- --ignored

    #[test]
    #[ignore = "requires a real terminal (TTY)"]
    fn guard_active_flag_toggled() {
        {
            let _guard = TerminalGuard::new().unwrap();
            assert!(GUARD_ACTIVE.load(Ordering::SeqCst));
        }
        assert!(!GUARD_ACTIVE.load(Ordering::SeqCst));
    }

    #[test]
    #[ignore = "requires a real terminal (TTY)"]
    fn guard_double_drop_is_safe() {
        let mut guard = TerminalGuard::new().unwrap();
        guard.restore(); // First cleanup
        guard.restore(); // Second cleanup should be no-op
        drop(guard); // Drop calls restore again — should be safe
        assert!(!GUARD_ACTIVE.load(Ordering::SeqCst));
    }

    #[test]
    #[ignore = "requires a real terminal (TTY)"]
    fn guard_restores_on_panic_unwind() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = TerminalGuard::new().unwrap();
            panic!("simulated panic");
        }));
        assert!(result.is_err());
        assert!(!GUARD_ACTIVE.load(Ordering::SeqCst));
    }

    #[test]
    #[ignore = "requires a real terminal (TTY)"]
    fn guard_multiple_instances_sequential() {
        {
            let _g1 = TerminalGuard::new().unwrap();
            assert!(GUARD_ACTIVE.load(Ordering::SeqCst));
        }
        assert!(!GUARD_ACTIVE.load(Ordering::SeqCst));
        {
            let _g2 = TerminalGuard::new().unwrap();
            assert!(GUARD_ACTIVE.load(Ordering::SeqCst));
        }
        assert!(!GUARD_ACTIVE.load(Ordering::SeqCst));
    }
}
