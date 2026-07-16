use std::io::{self, Write};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::mpsc::UnboundedSender;

use super::super::{
    approval::start_pending_approval_task,
    chat::TuiTaskHandle,
    data::{
        TuiActivityItem, TuiActivityKind, TuiFocusPanel, TuiSelectionPoint, TuiState, TuiUpdate,
    },
    layout::{TuiLayout, TuiResizeHandle, contains},
    render::{
        TuiContextClickAction, approval_selection_at_position, completion_index_at_position,
        context_action_for_click, input_cursor_for_click, input_panel_height,
        selected_text_for_layout,
    },
};
use super::push_command_error;

#[cfg(test)]
pub(super) fn handle_mouse_event(
    state: &mut TuiState,
    mouse: MouseEvent,
    terminal_width: u16,
    terminal_height: u16,
    sender: &UnboundedSender<TuiUpdate>,
    active_task: &mut Option<TuiTaskHandle>,
) {
    let mut mouse_drag = None;
    handle_mouse_event_with_drag(
        state,
        mouse,
        terminal_width,
        terminal_height,
        sender,
        active_task,
        &mut mouse_drag,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TuiMouseDrag {
    Resize(TuiResizeHandle),
    Select(TuiFocusPanel),
}

pub(super) fn handle_mouse_event_with_drag(
    state: &mut TuiState,
    mouse: MouseEvent,
    terminal_width: u16,
    terminal_height: u16,
    sender: &UnboundedSender<TuiUpdate>,
    active_task: &mut Option<TuiTaskHandle>,
    mouse_drag: &mut Option<TuiMouseDrag>,
) {
    let layout = mouse_layout(state, terminal_width, terminal_height);
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            scroll_at_position(state, &layout, mouse.column, mouse.row, true)
        }
        MouseEventKind::ScrollDown => {
            scroll_at_position(state, &layout, mouse.column, mouse.row, false)
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if state.pending_approval.is_some() {
                let area = Rect::new(0, 0, terminal_width, terminal_height);
                if let Some(selection) =
                    approval_selection_at_position(area, mouse.column, mouse.row)
                {
                    match start_pending_approval_task(
                        state,
                        selection,
                        selection.command(),
                        sender.clone(),
                    ) {
                        Ok(task) => *active_task = Some(task),
                        Err(error) => push_command_error(state, error.to_string()),
                    }
                }
                return;
            }
            if state.completion.is_some() {
                let area = Rect::new(0, 0, terminal_width, terminal_height);
                if let Some(index) =
                    completion_index_at_position(state, area, layout.input, mouse.column, mouse.row)
                {
                    state.select_completion(index);
                    state.accept_completion();
                    return;
                }
                state.clear_completions();
            }
            if let Some(panel) = sidebar_tab_at_position(&layout, mouse.column, mouse.row) {
                state.focus_panel(panel);
                state.clear_text_selection();
                *mouse_drag = None;
                return;
            }
            if let Some(handle) = layout.resize_handle_at(mouse.column, mouse.row) {
                *mouse_drag = Some(TuiMouseDrag::Resize(handle));
                resize_panes(state, layout, handle, mouse.column, mouse.row);
                state.clear_text_selection();
                return;
            }
            if let Some(panel) = layout.focus_panel_at(mouse.column, mouse.row) {
                state.begin_text_selection(
                    panel,
                    selection_point_for_panel(layout, panel, mouse.column, mouse.row),
                );
                *mouse_drag = Some(TuiMouseDrag::Select(panel));
            } else {
                *mouse_drag = None;
                state.clear_text_selection();
            }
            handle_mouse_click(state, &layout, mouse.column, mouse.row)
        }
        MouseEventKind::Drag(MouseButton::Left) => match *mouse_drag {
            Some(TuiMouseDrag::Resize(handle)) => {
                resize_panes(state, layout, handle, mouse.column, mouse.row);
            }
            Some(TuiMouseDrag::Select(panel)) => {
                state.update_text_selection(
                    panel,
                    selection_point_for_panel(layout, panel, mouse.column, mouse.row),
                );
            }
            None => {}
        },
        MouseEventKind::Up(MouseButton::Left) => {
            if matches!(mouse_drag.take(), Some(TuiMouseDrag::Select(_)))
                && state.finish_text_selection()
                && let Some(text) = selected_text_for_layout(state, layout)
            {
                copy_selected_text(state, &text);
            }
        }
        _ => {}
    }
}

pub(super) fn mouse_layout(
    state: &TuiState,
    terminal_width: u16,
    terminal_height: u16,
) -> TuiLayout {
    let area = Rect::new(0, 0, terminal_width, terminal_height);
    TuiLayout::new(
        area,
        state.pane_sizing,
        input_panel_height(state, area),
        state.focused_panel,
        state.sidebar_panel,
    )
}

fn resize_panes(
    state: &mut TuiState,
    layout: TuiLayout,
    handle: TuiResizeHandle,
    column: u16,
    row: u16,
) {
    let sizing = layout.resize(handle, column, row);
    state.pane_sizing.side_width = sizing.side_width;
}

fn selection_point_for_panel(
    layout: TuiLayout,
    panel: TuiFocusPanel,
    column: u16,
    row: u16,
) -> TuiSelectionPoint {
    let area = match panel {
        TuiFocusPanel::Chat => layout.chat,
        TuiFocusPanel::Context => layout.context,
        TuiFocusPanel::Activity => layout.activity,
    };
    let content_x = area.x.saturating_add(1);
    let content_y = area.y.saturating_add(1);
    let content_width = area.width.saturating_sub(2);
    let content_height = area.height.saturating_sub(2);
    TuiSelectionPoint::new(
        column.saturating_sub(content_x).min(content_width),
        row.saturating_sub(content_y)
            .min(content_height.saturating_sub(1)),
    )
}

fn copy_selected_text(state: &mut TuiState, text: &str) {
    match write_osc52_clipboard(text) {
        Ok(()) => state.push_activity(TuiActivityItem::with_detail(
            TuiActivityKind::System,
            "selection copied",
            format!("{} chars", text.chars().count()),
        )),
        Err(error) => state.push_activity(TuiActivityItem::with_detail(
            TuiActivityKind::Error,
            "selection copy failed",
            error.to_string(),
        )),
    }
}

fn write_osc52_clipboard(text: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    write!(stdout, "\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))?;
    stdout.flush()
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        let triple = ((a as u32) << 16) | ((b as u32) << 8) | c as u32;
        output.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        output.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

fn scroll_at_position(state: &mut TuiState, layout: &TuiLayout, column: u16, row: u16, up: bool) {
    if contains(layout.input, column, row) {
        state.input_mode = true;
        if up {
            state.history_previous();
        } else {
            state.history_next();
        }
    } else if contains(layout.context, column, row) {
        state.focus_panel(TuiFocusPanel::Context);
        if up {
            state.scroll_context_up();
        } else {
            state.scroll_context_down();
        }
    } else if contains(layout.activity, column, row) {
        state.focus_panel(TuiFocusPanel::Activity);
        if up {
            state.scroll_activity_up();
        } else {
            state.scroll_activity_down();
        }
    } else if contains(layout.chat, column, row) {
        state.focus_panel(TuiFocusPanel::Chat);
        if up {
            state.scroll_chat_up();
        } else {
            state.scroll_chat_down();
        }
    }
}

fn handle_mouse_click(state: &mut TuiState, layout: &TuiLayout, column: u16, row: u16) {
    if contains(layout.input, column, row) {
        state.input_mode = true;
        if let Some(cursor) = input_cursor_for_click(state, layout.input, column, row) {
            state.set_input_cursor(cursor);
        }
    }
    if let Some(panel) = layout.focus_panel_at(column, row) {
        state.focus_panel(panel);
    }
    if contains(layout.context, column, row)
        && let Some(action) = context_action_for_click(state, layout.context, column, row)
    {
        handle_context_click_action(state, action);
    }
    if contains(layout.activity, column, row)
        && let Some(run_id) = activity_recent_run_id_at_position(state, layout.activity, row)
    {
        state.input_mode = true;
        state.replace_command_input(format!("/inspect {run_id}"));
        state.push_activity(TuiActivityItem::with_detail(
            TuiActivityKind::System,
            "run selected",
            run_id,
        ));
    }
}

fn sidebar_tab_at_position(layout: &TuiLayout, column: u16, row: u16) -> Option<TuiFocusPanel> {
    let side = if layout.context.width > 0 {
        layout.context
    } else {
        layout.activity
    };
    if side.width == 0 || row != side.y || column <= side.x {
        return None;
    }
    let x = column.saturating_sub(side.x + 1);
    if x < 9 {
        Some(TuiFocusPanel::Context)
    } else if x < 21 {
        Some(TuiFocusPanel::Activity)
    } else {
        None
    }
}

fn handle_context_click_action(state: &mut TuiState, action: TuiContextClickAction) {
    match action {
        TuiContextClickAction::InspectProposal(proposal_id) => {
            state.input_mode = true;
            state.replace_command_input(format!("/proposal {proposal_id}"));
            state.push_activity(TuiActivityItem::with_detail(
                TuiActivityKind::System,
                "proposal selected",
                proposal_id,
            ));
        }
    }
}

fn activity_recent_run_id_at_position(
    state: &TuiState,
    activity: Rect,
    row: u16,
) -> Option<String> {
    if activity.height <= 2 || row <= activity.y || row >= activity.y + activity.height - 1 {
        return None;
    }
    let visible_row = usize::from(row - activity.y - 1);
    let height = activity.height.saturating_sub(2) as usize;
    let activity_count = if state.activity.is_empty() {
        1
    } else {
        state.activity.len()
    };
    let shown_runs = state.recent_runs.len().min(4);
    if shown_runs == 0 {
        return None;
    }
    let full_len = activity_count + 2 + shown_runs;
    let scroll = usize::from(state.event_scroll).min(full_len.saturating_sub(height));
    let end = full_len.saturating_sub(scroll);
    let start = end.saturating_sub(height);
    let full_index = start.saturating_add(visible_row);
    if full_index >= end {
        return None;
    }
    let recent_start = activity_count + 2;
    let run_index = full_index.checked_sub(recent_start)?;
    state
        .recent_runs
        .get(run_index)
        .filter(|_| run_index < shown_runs)
        .map(|run| run.run_id.0.clone())
}
