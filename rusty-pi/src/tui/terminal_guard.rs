//! Testable RAII guard for terminal state restoration.

use crossterm::execute;
use std::io::{self, Stdout};

/// The terminal operations needed by the TUI lifecycle.
///
/// The production implementation delegates to crossterm. Tests can inject a
/// fake implementation and observe the exact initialization/restoration
/// sequence without requiring a real TTY.
pub trait TerminalControl {
    fn enable_raw_mode(&mut self) -> io::Result<()>;
    fn disable_raw_mode(&mut self) -> io::Result<()>;
    fn enter_alternate_screen(&mut self) -> io::Result<()>;
    fn leave_alternate_screen(&mut self) -> io::Result<()>;
    fn hide_cursor(&mut self) -> io::Result<()>;
    fn show_cursor(&mut self) -> io::Result<()>;
}

/// Production terminal controller backed by crossterm and stdout.
pub struct CrosstermTerminal {
    stdout: Stdout,
}

impl CrosstermTerminal {
    pub fn new() -> Self {
        Self { stdout: io::stdout() }
    }

    /// Access the stdout used for terminal control.
    pub fn stdout_mut(&mut self) -> &mut Stdout {
        &mut self.stdout
    }
}

impl Default for CrosstermTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalControl for CrosstermTerminal {
    fn enable_raw_mode(&mut self) -> io::Result<()> {
        crossterm::terminal::enable_raw_mode()
    }

    fn disable_raw_mode(&mut self) -> io::Result<()> {
        crossterm::terminal::disable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> io::Result<()> {
        execute!(self.stdout, crossterm::terminal::EnterAlternateScreen)
    }

    fn leave_alternate_screen(&mut self) -> io::Result<()> {
        execute!(self.stdout, crossterm::terminal::LeaveAlternateScreen)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(self.stdout, crossterm::cursor::Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(self.stdout, crossterm::cursor::Show)
    }
}

/// RAII owner for terminal state.
///
/// `restore()` is idempotent. Dropping a guard after explicit restoration is
/// safe. Drop is best effort and never panics, including while unwinding.
pub struct TerminalGuard<C: TerminalControl> {
    control: C,
    raw_mode: bool,
    alternate_screen: bool,
    cursor_hidden: bool,
    restored: bool,
}

impl<C: TerminalControl> TerminalGuard<C> {
    /// Initialize the terminal and retain only the states that succeeded.
    ///
    /// If initialization fails part-way through, already-enabled state is
    /// restored before the error is returned.
    pub fn new(control: C) -> io::Result<Self> {
        let mut guard = Self {
            control,
            raw_mode: false,
            alternate_screen: false,
            cursor_hidden: false,
            restored: false,
        };

        guard.control.enable_raw_mode()?;
        guard.raw_mode = true;

        if let Err(error) = guard.control.enter_alternate_screen() {
            let _ = guard.restore();
            return Err(error);
        }
        guard.alternate_screen = true;

        if let Err(error) = guard.control.hide_cursor() {
            let _ = guard.restore();
            return Err(error);
        }
        guard.cursor_hidden = true;

        Ok(guard)
    }

    /// Restore terminal state once, continuing through all cleanup steps.
    ///
    /// The first cleanup error is returned, but later cleanup operations still
    /// run. The guard is marked restored before executing cleanup so a second
    /// call and Drop cannot repeat any operation.
    pub fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;

        let mut first_error = None;
        if self.cursor_hidden {
            if let Err(error) = self.control.show_cursor() {
                first_error = Some(error);
            }
            self.cursor_hidden = false;
        }
        if self.alternate_screen {
            if let Err(error) = self.control.leave_alternate_screen()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
            self.alternate_screen = false;
        }
        if self.raw_mode {
            if let Err(error) = self.control.disable_raw_mode()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
            self.raw_mode = false;
        }

        first_error.map_or(Ok(()), Err)
    }

    /// Borrow the production/test controller.
    pub fn control_mut(&mut self) -> &mut C {
        &mut self.control
    }
}

impl<C: TerminalControl> Drop for TerminalGuard<C> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

    #[derive(Clone, Default)]
    struct FakeTerminal {
        calls: Rc<RefCell<Vec<&'static str>>>,
        fail: Option<&'static str>,
    }

    impl FakeTerminal {
        fn with_failure(fail: &'static str) -> Self {
            Self {
                calls: Rc::new(RefCell::new(Vec::new())),
                fail: Some(fail),
            }
        }

        fn call(&mut self, name: &'static str) -> io::Result<()> {
            self.calls.borrow_mut().push(name);
            if self.fail == Some(name) {
                Err(io::Error::other(format!("fake failure: {name}")))
            } else {
                Ok(())
            }
        }
    }

    impl TerminalControl for FakeTerminal {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.call("enable_raw_mode")
        }
        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.call("disable_raw_mode")
        }
        fn enter_alternate_screen(&mut self) -> io::Result<()> {
            self.call("enter_alternate_screen")
        }
        fn leave_alternate_screen(&mut self) -> io::Result<()> {
            self.call("leave_alternate_screen")
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            self.call("hide_cursor")
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            self.call("show_cursor")
        }
    }

    #[test]
    fn normal_creation_and_drop_restores_in_reverse_order() {
        let fake = FakeTerminal::default();
        let calls = fake.calls.clone();
        let guard = TerminalGuard::new(fake).unwrap();
        assert_eq!(
            calls.borrow().as_slice(),
            &["enable_raw_mode", "enter_alternate_screen", "hide_cursor"]
        );
        drop(guard);
        assert_eq!(
            calls.borrow().as_slice(),
            &[
                "enable_raw_mode",
                "enter_alternate_screen",
                "hide_cursor",
                "show_cursor",
                "leave_alternate_screen",
                "disable_raw_mode",
            ]
        );
    }

    #[test]
    fn explicit_restore_then_drop_restores_once() {
        let fake = FakeTerminal::default();
        let calls = fake.calls.clone();
        let mut guard = TerminalGuard::new(fake).unwrap();
        guard.restore().unwrap();
        guard.restore().unwrap();
        drop(guard);
        assert_eq!(calls.borrow().iter().filter(|&&call| call == "show_cursor").count(), 1);
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|&&call| call == "leave_alternate_screen")
                .count(),
            1
        );
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|&&call| call == "disable_raw_mode")
                .count(),
            1
        );
    }

    #[test]
    fn initialization_failure_restores_only_successful_states() {
        for (failure, expected) in [
            ("enable_raw_mode", vec!["enable_raw_mode"]),
            (
                "enter_alternate_screen",
                vec!["enable_raw_mode", "enter_alternate_screen", "disable_raw_mode"],
            ),
            (
                "hide_cursor",
                vec![
                    "enable_raw_mode",
                    "enter_alternate_screen",
                    "hide_cursor",
                    "leave_alternate_screen",
                    "disable_raw_mode",
                ],
            ),
        ] {
            let fake = FakeTerminal::with_failure(failure);
            let calls = fake.calls.clone();
            assert!(TerminalGuard::new(fake).is_err());
            assert_eq!(calls.borrow().as_slice(), expected.as_slice(), "failure: {failure}");
        }
    }

    #[test]
    fn event_loop_error_drops_guard_and_restores() {
        fn failing_loop() -> io::Result<()> {
            let fake = FakeTerminal::default();
            let calls = fake.calls.clone();
            let result = Err(io::Error::other("event loop failed"));
            {
                let _guard = TerminalGuard::new(fake)?;
                assert!(calls.borrow().contains(&"enable_raw_mode"));
            }
            assert!(calls.borrow().contains(&"disable_raw_mode"));
            result
        }

        assert!(failing_loop().is_err());
    }

    #[test]
    fn panic_unwind_restores_terminal() {
        let fake = FakeTerminal::default();
        let calls = fake.calls.clone();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = TerminalGuard::new(fake).unwrap();
            panic!("simulated event-loop panic");
        }));
        assert!(result.is_err());
        assert_eq!(
            calls.borrow().as_slice(),
            &[
                "enable_raw_mode",
                "enter_alternate_screen",
                "hide_cursor",
                "show_cursor",
                "leave_alternate_screen",
                "disable_raw_mode",
            ]
        );
    }

    #[test]
    fn restore_failures_do_not_panic_during_drop() {
        let fake = FakeTerminal::with_failure("show_cursor");
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut guard = TerminalGuard::new(fake).unwrap();
            assert!(guard.restore().is_err());
        }));
        assert!(result.is_ok());

        let fake = FakeTerminal::with_failure("disable_raw_mode");
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = TerminalGuard::new(fake).unwrap();
        }));
        assert!(result.is_ok());
    }
}
