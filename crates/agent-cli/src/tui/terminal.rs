use std::{
    io::{self, Stdout},
    time::Duration,
};

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseEvent, MouseEventKind,
};
use miette::{IntoDiagnostic, Result};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::{
    approval::start_pending_approval_task,
    chat::{TuiTaskHandle, start_natural_language_task},
    commands::execute_command,
    data::{TuiActivityItem, TuiActivityKind, TuiApprovalSelection, TuiState, TuiUpdate},
    render::render_tui_frame,
};

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
                        KeyCode::Char(':') | KeyCode::Char('/') => state.enter_command("/"),
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
    key: KeyEvent,
    sender: &UnboundedSender<TuiUpdate>,
    active_task: &mut Option<TuiTaskHandle>,
) -> Result<bool> {
    let code = key.code;
    let modifiers = key.modifiers;
    match code {
        KeyCode::Esc => {
            if state.busy {
                cancel_active_task(state, active_task);
            } else if state.command_input.is_empty() {
                return Ok(true);
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
            if state.busy {
                state.push_activity(TuiActivityItem::new(
                    TuiActivityKind::System,
                    "still running; press Esc or Ctrl-C to cancel before sending",
                ));
                return Ok(false);
            }
            if state.approval_picker_active() {
                let selection = state.approval_selection;
                match start_pending_approval_task(
                    state,
                    selection,
                    selection.command(),
                    sender.clone(),
                ) {
                    Ok(task) => *active_task = Some(task),
                    Err(error) => push_command_error(state, error.to_string()),
                }
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
            state.history_previous();
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
        }
        KeyCode::Backspace => {
            state.backspace();
        }
        KeyCode::Delete => {
            state.delete();
        }
        KeyCode::Left if state.approval_picker_active() => {
            state.select_approval();
        }
        KeyCode::Right if state.approval_picker_active() => {
            state.select_denial();
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
        KeyCode::Up => state.history_previous(),
        KeyCode::Down => state.history_next(),
        KeyCode::PageUp => state.scroll_chat_up(),
        KeyCode::PageDown => state.scroll_chat_down(),
        KeyCode::Tab if state.approval_picker_active() => {
            state.toggle_approval_selection();
        }
        KeyCode::Tab => complete_slash_command(state),
        KeyCode::BackTab if state.approval_picker_active() => {
            state.toggle_approval_selection();
        }
        KeyCode::BackTab => {
            state.scroll_chat_up();
        }
        KeyCode::Char('g') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.scroll_chat_top();
        }
        KeyCode::Char('o') if modifiers.contains(KeyModifiers::CONTROL) => {
            state.scroll_chat_bottom();
        }
        KeyCode::Char(ch) if text_modifier(modifiers) => {
            state.insert_char(ch);
        }
        _ => {}
    }
    Ok(false)
}

async fn run_command(state: &mut TuiState, command: &str) {
    state.set_busy(true);
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

fn cancel_active_task(state: &mut TuiState, active_task: &mut Option<TuiTaskHandle>) {
    if let Some(task) = active_task.as_ref() {
        task.cancellation.cancel();
    }
    state.replace_active_assistant("Cancelling...");
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

fn complete_slash_command(state: &mut TuiState) {
    if state.input_cursor != state.command_input.len() {
        return;
    }
    let input = state.command_input.clone();
    if !input.starts_with('/') {
        return;
    }
    let body = &input[1..];
    if !body.contains(char::is_whitespace) {
        complete_slash_command_name(state, body);
        return;
    }

    let (verb, rest) = body
        .split_once(char::is_whitespace)
        .map(|(verb, rest)| (verb.trim(), rest.trim_start()))
        .unwrap_or((body.trim(), ""));
    if rest.contains(char::is_whitespace) {
        return;
    }
    match verb {
        "help" | "?" => {
            let candidates = help_topics();
            complete_slash_argument(
                state,
                verb,
                rest,
                candidates,
                "",
                "help topic",
                "help topics",
            );
        }
        "use" => {
            let candidates = agent_ids(state);
            complete_slash_argument(state, verb, rest, candidates, "", "agent id", "agent ids");
        }
        "run" => {
            let candidates = agent_ids(state);
            complete_slash_argument(state, verb, rest, candidates, " ", "agent id", "agent ids");
        }
        "tool" | "call" => {
            let candidates = tool_names(state);
            complete_slash_argument(
                state,
                verb,
                rest,
                candidates,
                " ",
                "tool name",
                "tool names",
            );
        }
        "inspect" | "events" | "cancel" | "proposals" => {
            let candidates = recent_run_ids(state);
            complete_slash_argument(state, verb, rest, candidates, "", "run id", "run ids");
        }
        "proposal" | "approve-proposal" | "deny-proposal" => {
            let candidates = proposal_ids(state);
            complete_slash_argument(
                state,
                verb,
                rest,
                candidates,
                "",
                "proposal id",
                "proposal ids",
            );
        }
        _ => {}
    }
}

fn complete_slash_command_name(state: &mut TuiState, typed: &str) {
    const COMMANDS: &[&str] = &[
        "help",
        "status",
        "agents",
        "use ",
        "tools",
        "clear",
        "refresh",
        "run ",
        "runs",
        "cancel ",
        "events ",
        "workflow ",
        "wf ",
        "proposals",
        "proposal ",
        "approve-proposal ",
        "deny-proposal ",
        "tool ",
        "approve",
        "yes",
        "deny",
        "no",
        "replay ",
        "inspect ",
    ];
    let matches = COMMANDS
        .iter()
        .copied()
        .filter(|command| command.starts_with(typed))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [command] => state.replace_command_input(format!("/{command}")),
        [] => state.push_event("no slash command matches"),
        commands => state.push_event(format!(
            "commands: {}",
            commands
                .iter()
                .map(|command| format!("/{command}"))
                .collect::<Vec<_>>()
                .join(" ")
        )),
    }
}

fn complete_slash_argument(
    state: &mut TuiState,
    verb: &str,
    typed: &str,
    candidates: Vec<String>,
    suffix: &str,
    singular: &str,
    plural: &str,
) {
    let matches = candidates
        .into_iter()
        .filter(|candidate| candidate.starts_with(typed))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [candidate] => state.replace_command_input(format!("/{verb} {candidate}{suffix}")),
        [] => state.push_event(format!("no {singular} matches")),
        candidates => state.push_event(format!(
            "{plural}: {}",
            candidates
                .iter()
                .map(|candidate| format!("/{verb} {candidate}"))
                .collect::<Vec<_>>()
                .join(" ")
        )),
    }
}

fn help_topics() -> Vec<String> {
    [
        "agents",
        "status",
        "use",
        "run",
        "runs",
        "inspect",
        "events",
        "workflow",
        "wf",
        "proposals",
        "proposal",
        "approve-proposal",
        "deny-proposal",
        "tool",
        "call",
        "approve",
        "yes",
        "y",
        "deny",
        "no",
        "n",
        "cancel",
        "trace",
        "replay",
        "refresh",
        "clear",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}

fn agent_ids(state: &TuiState) -> Vec<String> {
    let mut ids = Vec::new();
    for agent in &state.agents {
        if !ids.contains(&agent.id) {
            ids.push(agent.id.clone());
        }
    }
    ids
}

fn tool_names(state: &TuiState) -> Vec<String> {
    let mut names = Vec::new();
    let Some(inventory) = &state.tool_inventory else {
        return names;
    };
    for tool in &inventory.items {
        if !names.contains(&tool.name) {
            names.push(tool.name.clone());
        }
    }
    names
}

fn recent_run_ids(state: &TuiState) -> Vec<String> {
    let mut ids = Vec::new();
    for run in &state.recent_runs {
        if !ids.contains(&run.run_id.0) {
            ids.push(run.run_id.0.clone());
        }
    }
    ids
}

fn proposal_ids(state: &TuiState) -> Vec<String> {
    let mut ids = Vec::new();
    let Some(proposals) = &state.latest_proposals else {
        return ids;
    };
    for proposal in &proposals.proposals {
        if !ids.contains(&proposal.proposal_id) {
            ids.push(proposal.proposal_id.clone());
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{
        data::{
            TuiAgentSummary, TuiApprovalSelection, TuiPendingApproval, TuiProposalListSummary,
            TuiProposalSummary,
        },
        policy::TuiToolRisk,
        test_support::test_state,
    };
    use agent_core::{AgentRunRecord, AgentRunStatus, PROTOCOL_VERSION, RunId, RunScope};
    use serde_json::json;
    use time::OffsetDateTime;

    #[tokio::test]
    async fn tab_completes_recent_run_id_for_run_commands() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.recent_runs = vec![
            test_run("run_alpha", AgentRunStatus::Completed),
            test_run("run_beta", AgentRunStatus::Running),
        ];
        state.replace_command_input("/events run_a");

        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/events run_alpha");
        assert_eq!(state.input_cursor, state.command_input.len());
    }

    #[tokio::test]
    async fn tab_completes_agent_id_for_run_and_use_commands() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.agents = vec![TuiAgentSummary {
            id: "review_agent".to_owned(),
            name: "Review Agent".to_owned(),
        }];
        state.replace_command_input("/run review");

        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/run review_agent ");

        state.replace_command_input("/use review");
        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/use review_agent");
    }

    #[tokio::test]
    async fn tab_completes_tool_name_for_tool_commands() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.replace_command_input("/tool ech");

        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/tool echo ");
    }

    #[tokio::test]
    async fn tab_completes_help_topics() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.replace_command_input("/help ev");

        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/help events");

        state.replace_command_input("/? too");
        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/? tool");
    }

    #[tokio::test]
    async fn tab_lists_help_topic_candidates_when_ambiguous() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.replace_command_input("/help pro");

        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/help pro");
        assert!(state.events.iter().any(|event| {
            event.contains("help topics:")
                && event.contains("/help proposals")
                && event.contains("/help proposal")
        }));
    }

    #[tokio::test]
    async fn tab_lists_run_id_candidates_when_ambiguous() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.recent_runs = vec![
            test_run("run_alpha", AgentRunStatus::Completed),
            test_run("run_beta", AgentRunStatus::Running),
        ];
        state.replace_command_input("/cancel run_");

        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/cancel run_");
        assert!(state.events.iter().any(|event| {
            event.contains("run ids:")
                && event.contains("/cancel run_alpha")
                && event.contains("/cancel run_beta")
        }));
    }

    #[tokio::test]
    async fn tab_completes_latest_proposal_id_for_proposal_commands() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.latest_proposals = Some(TuiProposalListSummary {
            total_count: 1,
            pending_count: 1,
            approved_count: 0,
            denied_count: 0,
            proposals: vec![TuiProposalSummary {
                proposal_id: "proposal_alpha".to_owned(),
                run_id: "run_alpha".to_owned(),
                agent_id: "echo_agent".to_owned(),
                kind: "edit_file".to_owned(),
                summary: "Update a file".to_owned(),
                status: "pending_approval".to_owned(),
                risk: "High".to_owned(),
                diff_count: 1,
                warning_count: 0,
            }],
        });
        state.replace_command_input("/approve-proposal proposal_a");

        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/approve-proposal proposal_alpha");
    }

    #[tokio::test]
    async fn tab_completes_command_names_before_argument_mode() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.replace_command_input("/eve");

        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/events ");
    }

    #[tokio::test]
    async fn tab_completes_status_command_name() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.replace_command_input("/sta");

        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/status");
    }

    #[tokio::test]
    async fn tab_completes_approval_alias_command_names() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.replace_command_input("/ye");

        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/yes");

        state.replace_command_input("/no");
        complete_slash_command(&mut state);

        assert_eq!(state.command_input, "/no");
    }

    #[tokio::test]
    async fn approval_picker_keys_switch_selected_action() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.set_pending_approval(TuiPendingApproval::tool_call(
            "echo",
            TuiToolRisk::High,
            json!({}),
        ));
        let (sender, _receiver) = unbounded_channel();
        let mut active_task = None;

        assert_eq!(state.approval_selection, TuiApprovalSelection::Approve);

        handle_input_key(
            &mut state,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &sender,
            &mut active_task,
        )
        .await
        .expect("tab handled");
        assert_eq!(state.approval_selection, TuiApprovalSelection::Deny);

        handle_input_key(
            &mut state,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            &sender,
            &mut active_task,
        )
        .await
        .expect("left handled");
        assert_eq!(state.approval_selection, TuiApprovalSelection::Approve);

        handle_input_key(
            &mut state,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &sender,
            &mut active_task,
        )
        .await
        .expect("right handled");
        assert_eq!(state.approval_selection, TuiApprovalSelection::Deny);
    }

    #[tokio::test]
    async fn approval_picker_enter_confirms_selected_action() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.set_pending_approval(TuiPendingApproval::tool_call(
            "echo",
            TuiToolRisk::High,
            json!({"value": "skip"}),
        ));
        state.select_denial();
        let (sender, mut receiver) = unbounded_channel();
        let mut active_task = None;

        handle_input_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &sender,
            &mut active_task,
        )
        .await
        .expect("enter handled");

        assert!(state.pending_approval.is_none());
        assert!(state.busy);
        assert!(active_task.is_some());
        assert_eq!(state.approval_selection, TuiApprovalSelection::Approve);
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Approval && activity.title == "approval denied"
        }));
        assert!(state.transcript.iter().any(|item| item.content == "no"));

        apply_updates_until_idle(&mut state, &mut receiver).await;
        if let Some(task) = active_task.take() {
            task.join.await.expect("approval task joins");
        }

        assert!(
            !state
                .transcript
                .iter()
                .any(|item| item.content.contains("skip"))
        );
        assert!(
            state
                .transcript
                .iter()
                .any(|item| { item.content.contains("Denied high-risk tool 'echo'.") })
        );
    }

    #[tokio::test]
    async fn typed_approval_reply_starts_background_task() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.set_pending_approval(TuiPendingApproval::tool_call(
            "echo",
            TuiToolRisk::High,
            json!({"value": "typed"}),
        ));
        state.replace_command_input("no");
        let (sender, mut receiver) = unbounded_channel();
        let mut active_task = None;

        handle_input_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &sender,
            &mut active_task,
        )
        .await
        .expect("enter handled");

        assert!(state.busy);
        assert!(active_task.is_some());
        assert!(state.transcript.iter().any(|item| item.content == "no"));

        apply_updates_until_idle(&mut state, &mut receiver).await;
        if let Some(task) = active_task.take() {
            task.join.await.expect("approval task joins");
        }

        assert!(state.pending_approval.is_none());
        assert!(
            state
                .transcript
                .iter()
                .any(|item| { item.content.contains("Denied high-risk tool 'echo'.") })
        );
    }

    async fn apply_updates_until_idle(
        state: &mut TuiState,
        receiver: &mut UnboundedReceiver<TuiUpdate>,
    ) {
        loop {
            let update = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
                .await
                .expect("update arrives")
                .expect("update exists");
            state.apply_update(update);
            if !state.busy {
                break;
            }
        }
    }

    fn test_run(run_id: &str, status: AgentRunStatus) -> AgentRunRecord {
        let now = OffsetDateTime::now_utc();
        AgentRunRecord {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: RunId(run_id.to_owned()),
            idempotency_key: None,
            agent_id: "echo_agent".to_owned(),
            status: status.clone(),
            scope: RunScope::Global,
            started_at: now,
            finished_at: (status != AgentRunStatus::Running).then_some(now),
            input: json!({}),
            output: json!({}),
            error: None,
            workflow: None,
            metadata: json!({}),
        }
    }
}
