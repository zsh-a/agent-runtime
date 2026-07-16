use std::{
    io::{self, Stdout},
    time::Duration,
};

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use miette::{IntoDiagnostic, Result};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::{
    approval::start_pending_approval_task,
    chat::{TuiTaskHandle, start_natural_language_task},
    commands::execute_command,
    completion::{complete_slash_command, open_command_palette, refresh_command_palette},
    data::{TuiActivityItem, TuiActivityKind, TuiApprovalSelection, TuiState, TuiUpdate},
    render::render_tui_frame,
};

mod mouse;

use mouse::{TuiMouseDrag, handle_mouse_event_with_drag};

#[cfg(test)]
use mouse::{handle_mouse_event, mouse_layout};

pub(super) async fn run_tui_terminal(mut state: TuiState) -> Result<()> {
    let mouse_capture = state.options.mouse_capture;
    crossterm::terminal::enable_raw_mode().into_diagnostic()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen).into_diagnostic()?;
    if mouse_capture {
        crossterm::execute!(stdout, EnableMouseCapture).into_diagnostic()?;
    }
    let result = run_tui_event_loop(
        &mut Terminal::new(CrosstermBackend::new(stdout)).into_diagnostic()?,
        &mut state,
        mouse_capture,
    )
    .await;
    crossterm::terminal::disable_raw_mode().into_diagnostic()?;
    let mut stdout = io::stdout();
    if mouse_capture {
        crossterm::execute!(stdout, DisableMouseCapture).into_diagnostic()?;
    }
    crossterm::execute!(stdout, crossterm::terminal::LeaveAlternateScreen).into_diagnostic()?;
    result
}

async fn run_tui_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut TuiState,
    mouse_capture: bool,
) -> Result<()> {
    let (sender, mut receiver) = unbounded_channel();
    let mut active_task: Option<TuiTaskHandle> = None;
    let mut mouse_drag: Option<TuiMouseDrag> = None;
    loop {
        drain_updates(state, &mut receiver);
        if active_task
            .as_ref()
            .is_some_and(|task| task.join.is_finished())
        {
            active_task = None;
        }
        terminal
            .draw(|frame| render_tui_frame(frame, state))
            .into_diagnostic()?;
        if crossterm::event::poll(Duration::from_millis(100)).into_diagnostic()? {
            match crossterm::event::read().into_diagnostic()? {
                Event::Key(key) if key.kind == KeyEventKind::Release => {}
                Event::Key(key) => {
                    if state.input_mode {
                        if handle_input_key(state, key, &sender, &mut active_task).await? {
                            return Ok(());
                        }
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            open_command_palette(state)
                        }
                        KeyCode::Enter => state.enter_input_mode(),
                        KeyCode::Tab | KeyCode::Right => state.focus_next_panel(),
                        KeyCode::BackTab | KeyCode::Left => state.focus_previous_panel(),
                        KeyCode::PageUp | KeyCode::Char('k') => state.scroll_focused_panel_up(),
                        KeyCode::PageDown | KeyCode::Char('j') => state.scroll_focused_panel_down(),
                        KeyCode::Char(':') | KeyCode::Char('/') => open_command_palette(state),
                        KeyCode::Char('r') => state.enter_command("/run "),
                        KeyCode::Char('t') => state.enter_command("/tool "),
                        KeyCode::Char('p') => state.enter_command("/replay "),
                        KeyCode::Char('i') => state.enter_command("/inspect "),
                        KeyCode::Char('R') => {
                            if let Err(error) = state.refresh().await {
                                state.push_activity(TuiActivityItem::with_detail(
                                    TuiActivityKind::Error,
                                    "refresh failed",
                                    error.to_string(),
                                ));
                            } else {
                                state.push_activity(TuiActivityItem::new(
                                    TuiActivityKind::System,
                                    "refreshed catalog/trace/store",
                                ));
                            }
                        }
                        KeyCode::Char('?') => run_command(state, "/help").await,
                        KeyCode::Char(ch) => {
                            state.enter_command("");
                            state.insert_char(ch);
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) if mouse_capture => {
                    let size = terminal.size().into_diagnostic()?;
                    handle_mouse_event_with_drag(
                        state,
                        mouse,
                        size.width,
                        size.height,
                        &sender,
                        &mut active_task,
                        &mut mouse_drag,
                    );
                }
                _ => {}
            }
        }
    }
}

async fn handle_input_key(
    state: &mut TuiState,
    key: KeyEvent,
    sender: &UnboundedSender<TuiUpdate>,
    active_task: &mut Option<TuiTaskHandle>,
) -> Result<bool> {
    let code = key.code;
    let modifiers = key.modifiers;
    if state.approval_picker_active() {
        match code {
            KeyCode::Left => state.select_approval(),
            KeyCode::Right => state.select_denial(),
            KeyCode::Tab | KeyCode::BackTab => state.toggle_approval_selection(),
            KeyCode::Esc => {
                let task = start_pending_approval_task(
                    state,
                    TuiApprovalSelection::Deny,
                    "no".to_owned(),
                    sender.clone(),
                )?;
                *active_task = Some(task);
            }
            KeyCode::Enter => {
                let Some(selection) = state.approval_selection else {
                    state.push_activity(TuiActivityItem::new(
                        TuiActivityKind::Approval,
                        "select approve or deny",
                    ));
                    return Ok(false);
                };
                let task = start_pending_approval_task(
                    state,
                    selection,
                    selection.command().to_owned(),
                    sender.clone(),
                )?;
                *active_task = Some(task);
            }
            _ => {}
        }
        return Ok(false);
    }
    match code {
        KeyCode::Esc => {
            if state.completion.is_some() {
                state.clear_completions();
            } else if state.busy {
                cancel_active_task(state, active_task);
            } else if state.command_input.is_empty() {
                state.leave_input_mode();
            } else {
                state.clear_command_input();
            }
        }
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
            if state.busy {
                cancel_active_task(state, active_task);
            } else {
                return Ok(true);
            }
        }
        KeyCode::Enter if newline_modifier(modifiers) => state.insert_newline(),
        KeyCode::Enter => {
            if state.accept_completion() {
                return Ok(false);
            }
            if state.busy {
                state.push_activity(TuiActivityItem::new(
                    TuiActivityKind::System,
                    "still running; press Esc or Ctrl-C to cancel before sending",
                ));
                return Ok(false);
            }
            let command = state.command_input.trim().to_owned();
            if command.is_empty() {
                state.clear_command_input();
                return Ok(false);
            }
            let command = state.take_submitted_input();
            state.remember_input(command.clone());
            if command.starts_with('/') {
                run_command(state, &command).await;
            } else if state.pending_approval.is_some()
                && let Some(selection) = approval_selection_from_reply(&command)
            {
                match start_pending_approval_task(state, selection, command.clone(), sender.clone())
                {
                    Ok(task) => *active_task = Some(task),
                    Err(error) => push_command_error(state, error.to_string()),
                }
            } else {
                *active_task = Some(start_natural_language_task(state, command, sender.clone()));
            }
        }
        KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => state.insert_newline(),
        KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.clear_output();
        }
        KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_cursor_to_start();
        }
        KeyCode::Char('e') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_cursor_to_end();
        }
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.delete_before_cursor();
        }
        KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.delete_after_cursor();
        }
        KeyCode::Char('w') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.delete_previous_word();
        }
        KeyCode::Char('p') if modifiers.contains(KeyModifiers::CONTROL) => {
            open_command_palette(state);
        }
        KeyCode::Char('n') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.history_next();
        }
        KeyCode::Char('b') if modifiers.contains(KeyModifiers::ALT) => {
            state.move_cursor_word_left();
        }
        KeyCode::Char('f') if modifiers.contains(KeyModifiers::ALT) => {
            state.move_cursor_word_right();
        }
        KeyCode::Backspace if modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) => {
            state.delete_previous_word();
            refresh_command_palette(state);
        }
        KeyCode::Backspace => {
            state.backspace();
            refresh_command_palette(state);
        }
        KeyCode::Delete => {
            state.delete();
            refresh_command_palette(state);
        }
        KeyCode::Left if modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) => {
            state.move_cursor_word_left();
        }
        KeyCode::Right if modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) => {
            state.move_cursor_word_right();
        }
        KeyCode::Left => {
            state.move_cursor_left();
        }
        KeyCode::Right => {
            state.move_cursor_right();
        }
        KeyCode::Home if modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_cursor_to_start();
        }
        KeyCode::End if modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_cursor_to_end();
        }
        KeyCode::Home => {
            state.move_cursor_to_line_start();
        }
        KeyCode::End => {
            state.move_cursor_to_line_end();
        }
        KeyCode::Up if state.completion.is_some() => {
            state.select_previous_completion();
        }
        KeyCode::Down if state.completion.is_some() => {
            state.select_next_completion();
        }
        KeyCode::Up => state.history_previous(),
        KeyCode::Down => state.history_next(),
        KeyCode::PageUp => state.scroll_focused_panel_up(),
        KeyCode::PageDown => state.scroll_focused_panel_down(),
        KeyCode::Tab => complete_slash_command(state),
        KeyCode::BackTab if state.select_previous_completion() => {}
        KeyCode::BackTab => {
            state.scroll_chat_up();
        }
        KeyCode::Char('g') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.scroll_chat_top();
        }
        KeyCode::Char('o') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.scroll_chat_bottom();
        }
        KeyCode::Char('/') if state.command_input.is_empty() && text_modifier(modifiers) => {
            open_command_palette(state);
        }
        KeyCode::Char(ch) if text_modifier(modifiers) => {
            state.insert_char(ch);
            refresh_command_palette(state);
        }
        _ => {}
    }
    Ok(false)
}

async fn run_command(state: &mut TuiState, command: &str) {
    state.start_operation("running command");
    if let Err(error) = execute_command(state, command).await {
        push_command_error(state, error.to_string());
    }
    state.set_busy(false);
}

fn push_command_error(state: &mut TuiState, error: String) {
    state.push_system_message(format!("Command failed: {error}"));
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Error,
        "command failed",
        error,
    ));
}

fn approval_selection_from_reply(input: &str) -> Option<TuiApprovalSelection> {
    match input.trim().to_ascii_lowercase().as_str() {
        "approve" | "ok" | "y" | "yes" => Some(TuiApprovalSelection::Approve),
        "cancel" | "deny" | "n" | "no" => Some(TuiApprovalSelection::Deny),
        _ => None,
    }
}

fn drain_updates(state: &mut TuiState, receiver: &mut UnboundedReceiver<TuiUpdate>) {
    while let Ok(update) = receiver.try_recv() {
        state.apply_update(update);
    }
}

fn cancel_active_task(state: &mut TuiState, active_task: &mut Option<TuiTaskHandle>) {
    if let Some(task) = active_task.as_ref() {
        task.cancellation.cancel();
    }
    state.replace_active_assistant("Cancelling...");
    state.set_operation_label("cancelling");
    state.push_activity(TuiActivityItem::new(
        TuiActivityKind::Cancellation,
        "cancellation requested",
    ));
}

fn newline_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL)
}

fn text_modifier(modifiers: KeyModifiers) -> bool {
    let command_modifiers =
        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::META;
    !modifiers.intersects(command_modifiers)
}

#[cfg(test)]
#[path = "tests/terminal.rs"]
mod tests;
