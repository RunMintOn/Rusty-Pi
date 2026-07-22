//! Injectable command driver for the TUI.
//!
//! The command future, terminal events, and redraw operation are separate
//! ports.  This keeps the production crossterm/Ratatui adapter thin while
//! allowing lifecycle tests to exercise the real concurrent polling loop.

use crate::coding_agent::command::{CommandControl, CommandOutcome};
use crate::tui::app::{Action, AppState, Effect};
use anyhow::Result;
use async_trait::async_trait;
use crossterm::event::Event;
use ratatui::Terminal;
use ratatui::backend::Backend;
use std::future::Future;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// The interval used to redraw while neither a command nor an input event is
/// ready.  A tick is observable in tests and keeps status changes responsive
/// even when the event source has no input.
pub const COMMAND_DRIVER_TICK: Duration = Duration::from_millis(40);

/// Source of terminal events for a command drive.
#[async_trait(?Send)]
pub trait CommandEventSource {
    /// Wait for the next event. `None` closes the source; the command remains
    /// active and is then driven by its own future and redraw ticks.
    async fn next_event(&mut self) -> Result<Option<Event>>;
}

/// Redraw boundary for the command driver.
pub trait CommandRenderer {
    fn redraw(&mut self, state: &AppState) -> Result<()>;
}

/// Why the command driver returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandDriveOutcome {
    Completed(CommandOutcome),
    Cancelled,
    Failed(String),
    Quit,
}

/// Drive one command while polling it, UI input, and redraw ticks together.
///
/// The command future is pinned locally and is never spawned or detached.  If
/// cancellation, a renderer error, or an event-source error occurs, the
/// command is still awaited before this function returns.
pub async fn drive_command_core<C, E, R>(
    command: C,
    mut events: E,
    mut renderer: R,
    state: &mut AppState,
    cancellation: CancellationToken,
) -> Result<CommandDriveOutcome>
where
    C: Future<Output = Result<CommandOutcome>>,
    E: CommandEventSource,
    R: CommandRenderer,
{
    tokio::pin!(command);
    let mut events_closed = false;

    loop {
        if let Err(error) = renderer.redraw(state) {
            cancellation.cancel();
            let _ = (&mut command).await;
            return Err(error);
        }

        tokio::select! {
            result = &mut command => {
                if cancellation.is_cancelled() {
                    state.update(Action::CommandCancelled);
                    return Ok(CommandDriveOutcome::Cancelled);
                }
                match result {
                    Ok(outcome) => {
                        let quit = outcome.control == CommandControl::Quit;
                        state.update(Action::CommandCompleted(outcome.clone()));
                        if quit || state.quit {
                            return Ok(CommandDriveOutcome::Quit);
                        }
                        return Ok(CommandDriveOutcome::Completed(outcome));
                    }
                    Err(error) => {
                        let message = error.to_string();
                        state.update(Action::CommandFailed(message.clone()));
                        return Ok(CommandDriveOutcome::Failed(message));
                    }
                }
            }
            event = events.next_event(), if !events_closed => {
                match event {
                    Ok(Some(event)) => {
                        let action = match event {
                            Event::Key(key) => Action::KeyInput(key),
                            Event::Paste(text) => Action::Paste(text),
                            Event::Resize(width, height) => Action::Resize(width, height),
                            _ => continue,
                        };
                        for effect in state.update(action) {
                            match effect {
                                Effect::CancelCommand => cancellation.cancel(),
                                Effect::Quit => cancellation.cancel(),
        Effect::SubmitInput(_)
        | Effect::SubmitControllerPrompt(_)
                                | Effect::CancelAgent => {
                                    // AppState rejects Enter while a command
                                    // is active, so no nested command/agent
                                    // can be started from this driver.
                                }
                            }
                        }
                    }
                    Ok(None) => events_closed = true,
                    Err(error) => {
                        cancellation.cancel();
                        let _ = (&mut command).await;
                        return Err(error);
                    }
                }
            }
            _ = tokio::time::sleep(COMMAND_DRIVER_TICK) => {}
        }
    }
}

/// Production crossterm event source. The blocking crossterm poll is kept at
/// the terminal adapter boundary and never appears in [`drive_command_core`].
#[derive(Debug, Clone, Copy)]
pub struct CrosstermCommandEventSource {
    poll_interval: Duration,
}

impl Default for CrosstermCommandEventSource {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(10),
        }
    }
}

impl CrosstermCommandEventSource {
    pub fn new(poll_interval: Duration) -> Self {
        Self { poll_interval }
    }
}

#[async_trait(?Send)]
impl CommandEventSource for CrosstermCommandEventSource {
    async fn next_event(&mut self) -> Result<Option<Event>> {
        loop {
            tokio::time::sleep(self.poll_interval).await;
            if crossterm::event::poll(Duration::ZERO)? {
                return Ok(Some(crossterm::event::read()?));
            }
        }
    }
}

/// Production Ratatui redraw adapter. It is generic over the backend so the
/// driver itself does not need to know how the terminal is connected.
pub struct RatatuiCommandRenderer<'a, B: Backend> {
    terminal: &'a mut Terminal<B>,
}

impl<'a, B: Backend> RatatuiCommandRenderer<'a, B> {
    pub fn new(terminal: &'a mut Terminal<B>) -> Self {
        Self { terminal }
    }
}

impl<B: Backend> CommandRenderer for RatatuiCommandRenderer<'_, B> {
    fn redraw(&mut self, state: &AppState) -> Result<()> {
        self.terminal.draw(|frame| crate::tui::app::view(frame, state))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ActivityState, TranscriptBlock};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::collections::VecDeque;
    use std::future::pending;
    use std::sync::{Arc, Mutex};
    use tokio::time::sleep;

    struct ScheduledEvents {
        events: VecDeque<(Duration, Event)>,
    }

    #[async_trait(?Send)]
    impl CommandEventSource for ScheduledEvents {
        async fn next_event(&mut self) -> Result<Option<Event>> {
            let Some((delay, event)) = self.events.pop_front() else {
                return Ok(pending().await);
            };
            sleep(delay).await;
            Ok(Some(event))
        }
    }

    #[derive(Clone, Default)]
    struct CountingRenderer {
        redraws: Arc<Mutex<usize>>,
        activities: Arc<Mutex<Vec<ActivityState>>>,
    }

    impl CommandRenderer for CountingRenderer {
        fn redraw(&mut self, state: &AppState) -> Result<()> {
            *self.redraws.lock().unwrap() += 1;
            self.activities.lock().unwrap().push(state.activity.clone());
            Ok(())
        }
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl_c() -> Event {
        Event::Key(ctrl_c_key())
    }

    fn ctrl_c_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    fn start_command(state: &mut AppState) {
        state.update(Action::CommandStarted {
            name: "delayed-test".into(),
        });
    }

    fn scheduled_lifecycle_events() -> ScheduledEvents {
        ScheduledEvents {
            events: VecDeque::from([
                (Duration::from_millis(5), key(KeyCode::Enter)),
                (Duration::from_millis(5), Event::Resize(120, 30)),
                (Duration::from_millis(5), key(KeyCode::PageUp)),
                (Duration::from_millis(5), ctrl_c()),
            ]),
        }
    }

    async fn delayed_command(
        cancellation: CancellationToken,
        observed_cancel: Arc<Mutex<bool>>,
        settled: Arc<Mutex<bool>>,
    ) -> Result<CommandOutcome> {
        loop {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    *observed_cancel.lock().unwrap() = true;
                    sleep(Duration::from_millis(20)).await;
                    *settled.lock().unwrap() = true;
                    return Ok(CommandOutcome::none());
                }
                _ = sleep(Duration::from_millis(5)) => {}
            }
        }
    }

    #[tokio::test]
    async fn driver_polls_pending_command_with_resize_navigation_ctrl_c_and_redraw() {
        let mut state = AppState::new((80, 24));
        start_command(&mut state);
        state.input.set_text("draft".into());
        let cancellation = CancellationToken::new();
        let observed_cancel = Arc::new(Mutex::new(false));
        let settled = Arc::new(Mutex::new(false));
        let renderer = CountingRenderer::default();
        let redraws = renderer.redraws.clone();
        let result = drive_command_core(
            delayed_command(cancellation.clone(), observed_cancel.clone(), settled.clone()),
            scheduled_lifecycle_events(),
            renderer,
            &mut state,
            cancellation.clone(),
        )
        .await
        .unwrap();

        assert_eq!(result, CommandDriveOutcome::Cancelled);
        assert!(*observed_cancel.lock().unwrap());
        assert!(*settled.lock().unwrap());
        assert!(cancellation.is_cancelled());
        assert!(*redraws.lock().unwrap() >= 1);
        assert_eq!(state.activity, ActivityState::Idle);
        assert_eq!(state.terminal_size, (120, 30));
        assert!(!state.scroll.follow_output, "PageUp must enter browsing mode");
        assert_eq!(state.input.text, "draft", "Enter cannot start another command");
        assert_eq!(state.current_run_id, None, "commands do not create Agent RunIds");
        assert!(
            state
                .transcript
                .iter()
                .all(|block| !matches!(block, TranscriptBlock::User { .. }))
        );
        assert_eq!(
            state
                .transcript
                .iter()
                .filter(|block| matches!(block, TranscriptBlock::System { message } if message == "Command cancelled"))
                .count(),
            1
        );

        // The driver has fully returned, so the next ordinary prompt can be
        // submitted through the same reducer without creating a command
        // transcript entry.
        state.update(Action::KeyInput(ctrl_c_key()));
        state.input.set_text("ordinary prompt".into());
        assert!(matches!(
            state.update(Action::Submit).as_slice(),
            [Effect::SubmitInput(input)] if input == "ordinary prompt"
        ));
    }

    #[tokio::test]
    async fn driver_waits_for_command_settlement_after_cancellation() {
        let mut state = AppState::new((80, 24));
        start_command(&mut state);
        let cancellation = CancellationToken::new();
        let settled = Arc::new(Mutex::new(false));
        let renderer = CountingRenderer::default();
        let result = drive_command_core(
            delayed_command(cancellation.clone(), Arc::new(Mutex::new(false)), settled.clone()),
            ScheduledEvents {
                events: VecDeque::from([(Duration::from_millis(5), ctrl_c())]),
            },
            renderer,
            &mut state,
            cancellation,
        )
        .await
        .unwrap();

        assert_eq!(result, CommandDriveOutcome::Cancelled);
        assert!(
            *settled.lock().unwrap(),
            "driver returned only after command settlement"
        );
        assert_eq!(state.activity, ActivityState::Idle);
    }

    #[tokio::test]
    async fn driver_redraws_on_no_event_ticks_until_command_finishes() {
        let mut state = AppState::new((80, 24));
        start_command(&mut state);
        let renderer = CountingRenderer::default();
        let redraws = renderer.redraws.clone();
        let command = async {
            sleep(Duration::from_millis(95)).await;
            Ok(CommandOutcome::message("done"))
        };
        let result = drive_command_core(
            command,
            ScheduledEvents {
                events: VecDeque::new(),
            },
            renderer,
            &mut state,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result, CommandDriveOutcome::Completed(_)));
        assert!(
            *redraws.lock().unwrap() >= 2,
            "tick redraws must happen while command is pending"
        );
        assert_eq!(state.activity, ActivityState::Idle);
        assert!(matches!(state.transcript.as_slice(), [TranscriptBlock::System { message }] if message == "done"));
    }
}
