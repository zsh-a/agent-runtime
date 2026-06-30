use std::{
    io::{self, Stdout},
    time::Duration,
};

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseEvent,
    MouseEventKind,
};
use miette::{IntoDiagnostic, Result};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

use super::{
    commands::{execute_command, start_natural_language_task},
    data::{TuiState, TuiUpdate},
    render::render_tui_frame,
};

pub(super) async fn run_tui_terminal(mut state: TuiState) -> Result<()> {
    crossterm::terminal::enable_raw_mode().into_diagnostic()?;
    let mut stdout = io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        EnableMouseCapture
    )
    .into_diagnostic()?;
    let result = run_tui_event_loop(
        &mut Terminal::new(CrosstermBackend::new(stdout)).into_diagnostic()?,
        &mut state,
    )
    .await;
    crossterm::terminal::disable_raw_mode().into_diagnostic()?;
    let mut stdout = io::stdout();
    crossterm::execute!(
        stdout,
        DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen
    )
    .into_diagnostic()?;
    result
}

async fn run_tui_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut TuiState,
) -> Result<()> {
    let (sender, mut receiver) = unbounded_channel();
    let mut active_task: Option<JoinHandle<()>> = None;
    loop {
        drain_updates(state, &mut receiver);
        if active_task.as_ref().is_some_and(|task| task.is_finished()) {
            active_task = None;
        }
        terminal
            .draw(|frame| render_tui_frame(frame, state))
            .into_diagnostic()?;
        if crossterm::event::poll(Duration::from_millis(100)).into_diagnostic()? {
            match crossterm::event::read().into_diagnostic()? {
                Event::Key(key) => {
                    if state.input_mode {
                        if handle_input_key(
                            state,
                            key.code,
                            key.modifiers,
                            &sender,
                            &mut active_task,
                        )
                        .await?
                        {
                            return Ok(());
                        }
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char(':') | KeyCode::Char('/') => state.enter_command("/"),
                        KeyCode::Char('r') => state.enter_command("/run "),
                        KeyCode::Char('t') => state.enter_command("/tool "),
                        KeyCode::Char('p') => state.enter_command("/replay "),
                        KeyCode::Char('i') => state.enter_command("/inspect "),
                        KeyCode::Char('R') => {
                            if let Err(error) = state.refresh().await {
                                state.push_event(format!("refresh failed: {error}"));
                            } else {
                                state.push_event("refreshed catalog/trace/store");
                            }
                        }
                        KeyCode::Char('?') => run_command(state, "/help").await,
                        KeyCode::Char(ch) => {
                            state.enter_command("");
                            state.command_input.push(ch);
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    let width = terminal.size().into_diagnostic()?.width;
                    handle_mouse_event(state, mouse, width);
                }
                _ => {}
            }
        }
    }
}

async fn handle_input_key(
    state: &mut TuiState,
    code: KeyCode,
    modifiers: KeyModifiers,
    sender: &UnboundedSender<TuiUpdate>,
    active_task: &mut Option<JoinHandle<()>>,
) -> Result<bool> {
    match code {
        KeyCode::Esc => {
            if state.busy {
                if let Some(task) = active_task.take() {
                    task.abort();
                }
                state.replace_streaming_assistant("Cancelled.");
                state.set_busy(false);
                state.push_event("cancelled current task");
            } else if state.command_input.is_empty() {
                return Ok(true);
            } else {
                state.command_input.clear();
            }
        }
        KeyCode::Enter => {
            let command = state.command_input.trim().to_owned();
            state.command_input.clear();
            state.remember_input(command.clone());
            if command.starts_with('/') {
                run_command(state, &command).await;
            } else if !command.is_empty() && !state.busy {
                *active_task = Some(start_natural_language_task(state, command, sender.clone()));
            }
        }
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
        KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.clear_output();
        }
        KeyCode::Backspace => {
            state.command_input.pop();
        }
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.command_input.clear();
        }
        KeyCode::Up => state.history_previous(),
        KeyCode::Down => state.history_next(),
        KeyCode::PageUp => state.scroll_chat_up(),
        KeyCode::PageDown => state.scroll_chat_down(),
        KeyCode::Home if modifiers.contains(KeyModifiers::CONTROL) => {
            state.scroll_chat_top();
        }
        KeyCode::End if modifiers.contains(KeyModifiers::CONTROL) => {
            state.scroll_chat_bottom();
        }
        KeyCode::Char(ch) => state.command_input.push(ch),
        _ => {}
    }
    Ok(false)
}

async fn run_command(state: &mut TuiState, command: &str) {
    state.set_busy(true);
    if let Err(error) = execute_command(state, command).await {
        state.push_system_message(format!("Command failed: {error}"));
        state.push_event(format!("command failed: {error}"));
    }
    state.set_busy(false);
}

fn drain_updates(state: &mut TuiState, receiver: &mut UnboundedReceiver<TuiUpdate>) {
    while let Ok(update) = receiver.try_recv() {
        state.apply_update(update);
    }
}

fn handle_mouse_event(state: &mut TuiState, mouse: MouseEvent, terminal_width: u16) {
    match mouse.kind {
        MouseEventKind::ScrollUp => scroll_at_column(state, mouse.column, terminal_width, true),
        MouseEventKind::ScrollDown => scroll_at_column(state, mouse.column, terminal_width, false),
        _ => {}
    }
}

fn scroll_at_column(state: &mut TuiState, column: u16, terminal_width: u16, up: bool) {
    let side_panel_start = terminal_width.saturating_mul(72) / 100;
    if column >= side_panel_start {
        if up {
            state.scroll_activity_up();
        } else {
            state.scroll_activity_down();
        }
    } else if up {
        state.scroll_chat_up();
    } else {
        state.scroll_chat_down();
    }
}
