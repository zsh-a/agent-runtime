use miette::{IntoDiagnostic, Result};
use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{
    data::{
        TranscriptItem, TranscriptRole, TuiActivityItem, TuiActivityKind, TuiApprovalSelection,
        TuiFocusPanel, TuiProposalListSummary, TuiRunSummary, TuiState, TuiTextSelection,
        TuiTraceEventSummary, TuiWorkflowSummary,
    },
    layout::TuiLayout,
};

const MAX_INPUT_HEIGHT: u16 = 8;
const MIN_INPUT_HEIGHT: u16 = 3;
const INPUT_PREFIX_WIDTH: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TuiContextClickAction {
    InspectProposal(String),
}

struct ContextPanelItem {
    line: Line<'static>,
    action: Option<TuiContextClickAction>,
}

impl ContextPanelItem {
    fn inert(line: Line<'static>) -> Self {
        Self { line, action: None }
    }

    fn clickable(line: Line<'static>, action: TuiContextClickAction) -> Self {
        Self {
            line,
            action: Some(action),
        }
    }
}

pub(super) fn render_tui_once(state: &TuiState) -> Result<String> {
    let backend = TestBackend::new(110, 34);
    let mut terminal = Terminal::new(backend).into_diagnostic()?;
    terminal
        .draw(|frame| render_tui_frame(frame, state))
        .into_diagnostic()?;
    Ok(buffer_to_string(terminal.backend().buffer()))
}

pub(super) fn render_tui_frame(frame: &mut Frame<'_>, state: &TuiState) {
    let area = frame.area();
    let input_height = input_panel_height(state, area);
    let layout = TuiLayout::new(area, state.pane_sizing, input_height);

    frame.render_widget(status_line(state), layout.status);
    frame.render_widget(chat_panel(state, layout.chat), layout.chat);
    frame.render_widget(context_panel(state, layout.context), layout.context);
    frame.render_widget(activity_panel(state, layout.activity), layout.activity);

    let input = command_panel(state, layout.input);
    frame.render_widget(input.paragraph, layout.input);
    if state.input_mode && layout.input.width > 2 && layout.input.height > 2 {
        frame.set_cursor_position((
            layout.input.x + 1 + input.cursor_x,
            layout.input.y + 1 + input.cursor_y,
        ));
    }
}

fn status_line(state: &TuiState) -> Paragraph<'static> {
    let status = if state.busy { "running" } else { "ready" };
    Paragraph::new(Line::from(vec![
        Span::styled(
            "Agent Runtime",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {} / {}",
                state.options.chat.provider, state.options.chat.model
            ),
            Style::default().fg(Color::Gray),
        ),
        Span::raw("  |  "),
        Span::styled(status, busy_style(state.busy)),
        Span::raw("  |  "),
        Span::styled(state.status.clone(), Style::default().fg(Color::DarkGray)),
    ]))
}

fn context_panel(state: &TuiState, area: Rect) -> List<'static> {
    let items = visible_context_items(state, area);
    let lines = items
        .into_iter()
        .enumerate()
        .map(|(row, item)| {
            ListItem::new(highlight_selection_line(
                state,
                TuiFocusPanel::Context,
                row,
                item.line,
            ))
        })
        .collect::<Vec<_>>();
    let height = area.height.saturating_sub(2) as usize;
    let max_scroll = context_panel_items(state).len().saturating_sub(height);
    let scroll = usize::from(state.context_scroll).min(max_scroll);
    let title = context_panel_title(scroll, max_scroll);
    List::new(lines).block(focused_panel_block(state, TuiFocusPanel::Context, title))
}

pub(super) fn context_action_for_click(
    state: &TuiState,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<TuiContextClickAction> {
    if area.width <= 2 || area.height <= 2 {
        return None;
    }
    let content_x = area.x.saturating_add(1);
    let content_y = area.y.saturating_add(1);
    if column < content_x
        || column >= area.x.saturating_add(area.width).saturating_sub(1)
        || row < content_y
        || row >= area.y.saturating_add(area.height).saturating_sub(1)
    {
        return None;
    }

    let height = area.height.saturating_sub(2) as usize;
    let items = context_panel_items(state);
    let max_scroll = items.len().saturating_sub(height);
    let scroll = usize::from(state.context_scroll).min(max_scroll);
    let index = scroll.saturating_add(usize::from(row.saturating_sub(content_y)));
    items.get(index).and_then(|item| item.action.clone())
}

fn visible_context_items(state: &TuiState, area: Rect) -> Vec<ContextPanelItem> {
    let height = area.height.saturating_sub(2) as usize;
    let items = context_panel_items(state);
    let max_scroll = items.len().saturating_sub(height);
    let scroll = usize::from(state.context_scroll).min(max_scroll);
    top_window(items, height, scroll)
}

fn context_panel_items(state: &TuiState) -> Vec<ContextPanelItem> {
    let mut items = Vec::new();
    if let Some(summary) = &state.catalog_summary {
        items.extend([
            context_item(format!(
                "catalog {} agents / {} tools",
                summary.agent_count, summary.tool_count
            )),
            context_item(format!("domains {}", summary.active_domains.join(", "))),
        ]);
    } else {
        items.push(context_item("catalog: not loaded"));
    }
    items.push(context_item(format!(
        "agent: {}",
        state.active_agent_label()
    )));
    items.push(context_item(format!(
        "trace: {}",
        state
            .trace_label
            .clone()
            .unwrap_or_else(|| "not loaded".to_owned())
    )));
    if let Some(trace) = &state.trace {
        items.extend([
            context_item(format!("run {}", trace.run_id.0)),
            context_item(format!("agent {}@{}", trace.agent_id, trace.agent_version)),
            context_item(format!("events: {}", trace.events.len())),
        ]);
    }
    if let Some(inventory) = &state.tool_inventory {
        items.push(context_item(format!(
            "tools {} high {} blocked {}",
            inventory.total_count(),
            inventory.high_risk_count(),
            inventory.blocked_count()
        )));
    }
    if let Some(run) = &state.latest_run {
        items.extend(run_context_items(run));
    }
    if let Some(workflow) = &state.latest_workflow {
        items.extend(workflow_context_items(workflow));
    }
    if let Some(proposals) = &state.latest_proposals {
        items.extend(proposal_context_items(proposals));
    }
    if let Some(events) = &state.latest_events {
        items.extend(trace_event_context_items(events));
    }
    if let Some(context) = &state.context_status {
        items.push(context_item(""));
        items.push(context_item(Line::styled(
            "chat context",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        items.push(context_item(format!(
            "tokens {}/{}",
            context.token_estimate, context.max_input_tokens
        )));
        items.push(context_item(format!("blocks {}", context.block_count)));
        if context.compacted {
            let strategy = context.compaction_strategy.as_deref().unwrap_or("unknown");
            items.push(context_item(format!(
                "compacted: {} omitted ({strategy})",
                context.omitted_block_count
            )));
        } else {
            items.push(context_item("compacted: no"));
        }
    }
    if let Some(approval) = &state.pending_approval {
        items.push(context_item(""));
        items.push(context_item(Line::styled(
            "pending approval",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        items.push(context_item(approval.summary()));
        items.push(context_item("Tab selects, Enter confirms"));
    }
    items
}

fn context_item(content: impl Into<Line<'static>>) -> ContextPanelItem {
    ContextPanelItem::inert(content.into())
}

fn context_panel_title(scroll: usize, max_scroll: usize) -> String {
    if max_scroll == 0 {
        "Context".to_owned()
    } else if scroll == 0 {
        format!("Context  {} more below", max_scroll)
    } else {
        format!("Context  {scroll}/{max_scroll}")
    }
}

fn run_context_items(run: &TuiRunSummary) -> Vec<ContextPanelItem> {
    let mut items = vec![
        context_item(""),
        context_item(Line::styled(
            "inspected run",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        context_item(run.run_id.clone()),
        context_item(format!("status {}", run.status)),
        context_item(format!("agent {}", run.agent_id)),
        context_item(format!(
            "started {}",
            compact_render_text(&run.started_at, 24)
        )),
    ];
    if let Some(finished_at) = &run.finished_at {
        items.push(context_item(format!(
            "finished {}",
            compact_render_text(finished_at, 23)
        )));
    }
    if run.cancellation_requested {
        items.push(context_item("cancel requested"));
    }
    if let Some(error) = &run.error {
        items.push(context_item(format!(
            "error {}",
            compact_render_text(error, 24)
        )));
    }
    items.push(context_item(format!(
        "input {}",
        compact_render_text(&run.input_preview, 24)
    )));
    items.push(context_item(format!(
        "output {}",
        compact_render_text(&run.output_preview, 24)
    )));
    items
}

fn workflow_context_items(workflow: &TuiWorkflowSummary) -> Vec<ContextPanelItem> {
    let mut items = vec![
        context_item(""),
        context_item(Line::styled(
            "workflow",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        context_item(format!("{} [{}]", workflow.workflow_id, workflow.status)),
        context_item(format!(
            "nodes {} ok {} fail {} skip {}",
            workflow.node_count,
            workflow.completed_count,
            workflow.failed_count,
            workflow.skipped_count
        )),
    ];
    if workflow.compensation_count > 0 {
        items.push(context_item(format!(
            "compensations {}",
            workflow.compensation_count
        )));
    }
    items.extend(workflow.nodes.iter().take(5).map(|node| {
        let mut detail = format!("{} {}", node.node_id, node.status);
        if let Some(reason) = &node.reason {
            detail.push_str(" ");
            detail.push_str(reason);
        }
        if !node.blocked_dependencies.is_empty() {
            detail.push_str(" blocked=");
            detail.push_str(&node.blocked_dependencies.join(","));
        }
        context_item(detail)
    }));
    if workflow.nodes.len() > 5 {
        items.push(context_item(format!("+{} more", workflow.nodes.len() - 5)));
    }
    items
}

fn trace_event_context_items(events: &TuiTraceEventSummary) -> Vec<ContextPanelItem> {
    let mut items = vec![
        context_item(""),
        context_item(Line::styled(
            "events",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        context_item(format!(
            "{} {}/{}",
            events.run_id, events.shown_count, events.event_count
        )),
    ];
    items.extend(events.events.iter().take(4).map(|event| {
        let detail = event
            .detail
            .as_deref()
            .filter(|detail| !detail.is_empty())
            .map(|detail| format!(" {detail}"))
            .unwrap_or_default();
        context_item(format!("{}{}", event.kind, detail))
    }));
    if events.events.len() > 4 {
        items.push(context_item(format!("+{} more", events.events.len() - 4)));
    }
    items
}

fn proposal_context_items(proposals: &TuiProposalListSummary) -> Vec<ContextPanelItem> {
    let mut items = vec![
        context_item(""),
        context_item(Line::styled(
            "proposals",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        context_item(format!(
            "total {} pend {} ok {} deny {}",
            proposals.total_count,
            proposals.pending_count,
            proposals.approved_count,
            proposals.denied_count
        )),
    ];
    items.extend(proposals.proposals.iter().take(4).map(|proposal| {
        ContextPanelItem::clickable(
            Line::from(format!(
                "{} {} {}",
                proposal.proposal_id, proposal.status, proposal.kind
            )),
            TuiContextClickAction::InspectProposal(proposal.proposal_id.clone()),
        )
    }));
    if proposals.proposals.len() > 4 {
        items.push(context_item(format!(
            "+{} more",
            proposals.proposals.len() - 4
        )));
    }
    items
}

fn visible_chat_lines(state: &TuiState, area: Rect) -> Vec<Line<'static>> {
    let width = area.width.saturating_sub(2).max(1);
    let height = area.height.saturating_sub(2) as usize;
    bottom_window(chat_lines(state, width), height, state.chat_scroll)
}

fn chat_panel(state: &TuiState, area: Rect) -> Paragraph<'static> {
    let visible = visible_chat_lines(state, area)
        .into_iter()
        .enumerate()
        .map(|(row, line)| highlight_selection_line(state, TuiFocusPanel::Chat, row, line))
        .collect::<Vec<_>>();
    let title = if state.chat_scroll == 0 {
        "Chat".to_owned()
    } else {
        format!("Chat +{} lines", state.chat_scroll)
    };

    Paragraph::new(Text::from(visible)).block(focused_panel_block(
        state,
        TuiFocusPanel::Chat,
        title,
    ))
}

fn activity_panel(state: &TuiState, area: Rect) -> List<'static> {
    let visible = visible_activity_lines(state, area)
        .into_iter()
        .enumerate()
        .map(|(row, line)| {
            ListItem::new(highlight_selection_line(
                state,
                TuiFocusPanel::Activity,
                row,
                line,
            ))
        })
        .collect::<Vec<_>>();
    let title = if state.event_scroll == 0 {
        "Activity".to_owned()
    } else {
        format!("Activity +{} lines", state.event_scroll)
    };
    List::new(visible).block(focused_panel_block(state, TuiFocusPanel::Activity, title))
}

fn visible_activity_lines(state: &TuiState, area: Rect) -> Vec<Line<'static>> {
    let mut items = Vec::new();
    if state.activity.is_empty() {
        items.push(Line::styled(
            "no activity",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        items.extend(state.activity.iter().map(activity_item_line));
    }
    if !state.recent_runs.is_empty() {
        items.push(Line::from(""));
        items.push(Line::styled(
            "recent runs",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        items.extend(state.recent_runs.iter().take(4).map(|run| {
            Line::from(vec![
                Span::styled(run.run_id.0.clone(), Style::default().fg(Color::Cyan)),
                Span::raw(" "),
                Span::raw(run.agent_id.clone()),
                Span::styled(
                    format!(" {:?}", run.status),
                    Style::default().fg(Color::Gray),
                ),
            ])
        }));
    }

    let height = area.height.saturating_sub(2) as usize;
    bottom_window(items, height, state.event_scroll)
}

fn activity_item_line(activity: &TuiActivityItem) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            format!("{:<7}", activity.kind.label()),
            activity_kind_style(&activity.kind),
        ),
        Span::raw(" "),
        Span::styled(activity.title.clone(), Style::default().fg(Color::Gray)),
    ];
    if let Some(detail) = activity.detail.as_ref().filter(|detail| !detail.is_empty()) {
        spans.extend([
            Span::styled(": ", Style::default().fg(Color::DarkGray)),
            Span::styled(detail.clone(), Style::default().fg(Color::DarkGray)),
        ]);
    }
    Line::from(spans)
}

pub(super) fn selected_text_for_layout(state: &TuiState, layout: TuiLayout) -> Option<String> {
    let selection = state.text_selection.as_ref()?;
    if selection.is_empty() {
        return None;
    }
    let lines = visible_text_lines_for_panel(state, layout, selection.panel);
    selected_text_from_lines(&lines, selection)
}

fn visible_text_lines_for_panel(
    state: &TuiState,
    layout: TuiLayout,
    panel: TuiFocusPanel,
) -> Vec<String> {
    match panel {
        TuiFocusPanel::Chat => visible_chat_lines(state, layout.chat),
        TuiFocusPanel::Context => visible_context_items(state, layout.context)
            .into_iter()
            .map(|item| item.line)
            .collect(),
        TuiFocusPanel::Activity => visible_activity_lines(state, layout.activity),
    }
    .into_iter()
    .map(|line| line.to_string())
    .collect()
}

fn highlight_selection_line(
    state: &TuiState,
    panel: TuiFocusPanel,
    row: usize,
    line: Line<'static>,
) -> Line<'static> {
    let Some(selection) = state.text_selection.as_ref() else {
        return line;
    };
    if selection.panel != panel || selection.is_empty() {
        return line;
    }
    let Some((start_col, end_col)) = selected_columns_for_row(selection, row) else {
        return line;
    };
    let text = line.to_string();
    let (before, selected, after) = split_display_columns(&text, start_col, end_col);
    if selected.is_empty() {
        return line;
    }
    Line::from(vec![
        Span::raw(before),
        Span::styled(selected, selection_style()),
        Span::raw(after),
    ])
}

fn selected_text_from_lines(lines: &[String], selection: &TuiTextSelection) -> Option<String> {
    let (start, end) = selection.ordered_points();
    let mut selected = Vec::new();
    for row in start.row..=end.row {
        let Some(line) = lines.get(usize::from(row)) else {
            break;
        };
        let start_col = if row == start.row { start.column } else { 0 };
        let end_col = if row == end.row { end.column } else { u16::MAX };
        selected.push(
            split_display_columns(line, start_col, end_col)
                .1
                .trim_end()
                .to_owned(),
        );
    }
    let text = selected.join("\n").trim_end().to_owned();
    (!text.is_empty()).then_some(text)
}

fn selected_columns_for_row(selection: &TuiTextSelection, row: usize) -> Option<(u16, u16)> {
    let (start, end) = selection.ordered_points();
    let row = u16::try_from(row).ok()?;
    if row < start.row || row > end.row {
        return None;
    }
    let start_col = if row == start.row { start.column } else { 0 };
    let end_col = if row == end.row { end.column } else { u16::MAX };
    (end_col > start_col).then_some((start_col, end_col))
}

fn split_display_columns(text: &str, start_col: u16, end_col: u16) -> (String, String, String) {
    let mut before = String::new();
    let mut selected = String::new();
    let mut after = String::new();
    let mut col = 0u16;

    for ch in text.chars() {
        let width = ch.width().unwrap_or(0) as u16;
        let next_col = col.saturating_add(width);
        if next_col <= start_col {
            before.push(ch);
        } else if col >= end_col {
            after.push(ch);
        } else {
            selected.push(ch);
        }
        col = next_col;
    }

    (before, selected, after)
}

fn selection_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::Cyan)
}

struct InputPanel {
    paragraph: Paragraph<'static>,
    cursor_x: u16,
    cursor_y: u16,
}

fn command_panel(state: &TuiState, area: Rect) -> InputPanel {
    let inner_width = area.width.saturating_sub(2).max(1);
    let inner_height = area.height.saturating_sub(2) as usize;
    let built = input_lines(state, inner_width);
    let max_visible = inner_height.max(1);
    let start = if built.cursor_y >= max_visible {
        built.cursor_y + 1 - max_visible
    } else {
        0
    };
    let cursor_y = built.cursor_y.saturating_sub(start) as u16;
    let lines = built
        .lines
        .into_iter()
        .skip(start)
        .take(max_visible)
        .collect::<Vec<_>>();
    let title = if state.busy {
        "Input  Esc/Ctrl-C cancels"
    } else if state.pending_approval.is_some() {
        "Input  pending approval: Tab selects  Enter confirms"
    } else {
        "Input  Enter sends  Shift+Enter newline  Tab completes"
    };

    InputPanel {
        paragraph: Paragraph::new(Text::from(lines)).block(panel_block(title)),
        cursor_x: built.cursor_x.min(inner_width.saturating_sub(1)),
        cursor_y: cursor_y.min(area.height.saturating_sub(3)),
    }
}

struct BuiltInput {
    lines: Vec<Line<'static>>,
    cursor_x: u16,
    cursor_y: usize,
}

pub(super) fn input_cursor_for_click(
    state: &TuiState,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<usize> {
    if area.width <= 2 || area.height <= 2 {
        return None;
    }
    let content_x = area.x.saturating_add(1);
    let content_y = area.y.saturating_add(1);
    if column < content_x
        || column >= area.x.saturating_add(area.width).saturating_sub(1)
        || row < content_y
        || row >= area.y.saturating_add(area.height).saturating_sub(1)
    {
        return None;
    }

    let inner_width = area.width.saturating_sub(2).max(1);
    let inner_height = area.height.saturating_sub(2) as usize;
    let built = input_lines(state, inner_width);
    let start = if built.cursor_y >= inner_height.max(1) {
        built.cursor_y + 1 - inner_height.max(1)
    } else {
        0
    };
    let target_y = usize::from(row.saturating_sub(content_y)).saturating_add(start);
    let target_x = column
        .saturating_sub(content_x)
        .min(inner_width.saturating_sub(1));
    let rows = input_click_rows(state, inner_width);
    rows.get(target_y)
        .map(|row| row.cursor_for_column(target_x))
        .or_else(|| rows.last().map(InputClickRow::end_cursor))
}

fn input_lines(state: &TuiState, width: u16) -> BuiltInput {
    let slash = state.command_input.starts_with('/');
    let prompt = if slash { "/ " } else { "> " };
    let prompt_style = if slash {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    };
    let body = if slash {
        state.command_input.strip_prefix('/').unwrap_or("")
    } else {
        state.command_input.as_str()
    };
    let body_cursor = if slash {
        state.input_cursor.saturating_sub(1).min(body.len())
    } else {
        state.input_cursor.min(body.len())
    };

    if body.is_empty() {
        if state.pending_approval.is_some() && !slash {
            return approval_picker_lines(state, width);
        }
        let placeholder = if slash {
            "command, Tab completes"
        } else {
            "message, /status, or /help"
        };
        return BuiltInput {
            lines: vec![Line::from(vec![
                Span::styled(prompt.to_owned(), prompt_style),
                Span::styled(placeholder.to_owned(), Style::default().fg(Color::DarkGray)),
            ])],
            cursor_x: INPUT_PREFIX_WIDTH.min(width.saturating_sub(1)),
            cursor_y: 0,
        };
    }

    let continuation = "  ";
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_prefix = prompt;
    let mut current_col = INPUT_PREFIX_WIDTH.min(width);
    let mut cursor = None;
    let mut y = 0usize;

    for (byte_index, ch) in body.char_indices() {
        if byte_index == body_cursor {
            cursor = Some((current_col.min(width.saturating_sub(1)), y));
        }
        if ch == '\n' {
            push_input_line(
                &mut lines,
                current_prefix,
                prompt_style,
                std::mem::take(&mut current),
            );
            current_prefix = continuation;
            current_col = INPUT_PREFIX_WIDTH.min(width);
            y += 1;
            continue;
        }

        let char_width = ch.width().unwrap_or(0) as u16;
        if !current.is_empty() && current_col.saturating_add(char_width) > width {
            push_input_line(
                &mut lines,
                current_prefix,
                prompt_style,
                std::mem::take(&mut current),
            );
            current_prefix = continuation;
            current_col = INPUT_PREFIX_WIDTH.min(width);
            y += 1;
        }
        current.push(ch);
        current_col = current_col.saturating_add(char_width);
        if current_col >= width {
            push_input_line(
                &mut lines,
                current_prefix,
                prompt_style,
                std::mem::take(&mut current),
            );
            current_prefix = continuation;
            current_col = INPUT_PREFIX_WIDTH.min(width);
            y += 1;
        }
    }

    if body.len() == body_cursor {
        cursor = Some((current_col.min(width.saturating_sub(1)), y));
    }
    push_input_line(&mut lines, current_prefix, prompt_style, current);
    let (cursor_x, cursor_y) = cursor.unwrap_or((INPUT_PREFIX_WIDTH.min(width), 0));

    BuiltInput {
        lines,
        cursor_x,
        cursor_y,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InputClickRow {
    start_cursor: usize,
    end_cursor: usize,
    end_col: u16,
    segments: Vec<InputClickSegment>,
}

impl InputClickRow {
    fn new(start_cursor: usize) -> Self {
        Self {
            start_cursor,
            end_cursor: start_cursor,
            end_col: INPUT_PREFIX_WIDTH,
            segments: Vec::new(),
        }
    }

    fn cursor_for_column(&self, column: u16) -> usize {
        if column <= INPUT_PREFIX_WIDTH {
            return self.start_cursor;
        }
        for segment in &self.segments {
            if column < segment.end_col {
                let midpoint = segment
                    .start_col
                    .saturating_add((segment.end_col.saturating_sub(segment.start_col)) / 2);
                return if column < midpoint {
                    segment.before_cursor
                } else {
                    segment.after_cursor
                };
            }
        }
        self.end_cursor
    }

    fn end_cursor(&self) -> usize {
        self.end_cursor
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InputClickSegment {
    start_col: u16,
    end_col: u16,
    before_cursor: usize,
    after_cursor: usize,
}

fn input_click_rows(state: &TuiState, width: u16) -> Vec<InputClickRow> {
    let slash = state.command_input.starts_with('/');
    let body = if slash {
        state.command_input.strip_prefix('/').unwrap_or("")
    } else {
        state.command_input.as_str()
    };
    let cursor_offset = usize::from(slash);
    if body.is_empty() {
        return vec![InputClickRow::new(state.command_input.len())];
    }

    let mut rows = Vec::new();
    let mut row = InputClickRow::new(cursor_offset);
    row.end_col = INPUT_PREFIX_WIDTH.min(width);

    for (byte_index, ch) in body.char_indices() {
        let before_cursor = cursor_offset + byte_index;
        let after_cursor = before_cursor + ch.len_utf8();
        if ch == '\n' {
            row.end_cursor = before_cursor;
            rows.push(row);
            row = InputClickRow::new(after_cursor);
            row.end_col = INPUT_PREFIX_WIDTH.min(width);
            continue;
        }

        let char_width = ch.width().unwrap_or(0) as u16;
        if !row.segments.is_empty() && row.end_col.saturating_add(char_width) > width {
            row.end_cursor = before_cursor;
            rows.push(row);
            row = InputClickRow::new(before_cursor);
            row.end_col = INPUT_PREFIX_WIDTH.min(width);
        }

        if char_width > 0 {
            let start_col = row.end_col;
            row.end_col = row.end_col.saturating_add(char_width);
            row.segments.push(InputClickSegment {
                start_col,
                end_col: row.end_col,
                before_cursor,
                after_cursor,
            });
        }
        row.end_cursor = after_cursor;

        if row.end_col >= width {
            rows.push(row);
            row = InputClickRow::new(after_cursor);
            row.end_col = INPUT_PREFIX_WIDTH.min(width);
        }
    }

    row.end_cursor = cursor_offset + body.len();
    rows.push(row);
    rows
}

fn push_input_line(
    lines: &mut Vec<Line<'static>>,
    prefix: &str,
    prompt_style: Style,
    content: String,
) {
    lines.push(Line::from(vec![
        Span::styled(prefix.to_owned(), prompt_style),
        Span::raw(content),
    ]));
}

fn chat_lines(state: &TuiState, width: u16) -> Vec<Line<'static>> {
    if state.transcript.is_empty() {
        return vec![Line::from(Span::styled(
            "No messages yet.",
            Style::default().fg(Color::DarkGray),
        ))];
    }

    let mut lines = Vec::new();
    for (index, item) in state.transcript.iter().enumerate() {
        if index > 0 {
            lines.push(Line::from(""));
        }
        push_transcript_item(&mut lines, item, width);
    }
    lines
}

fn push_transcript_item(lines: &mut Vec<Line<'static>>, item: &TranscriptItem, width: u16) {
    let mut title = item.role.label().to_owned();
    if let Some(extra) = &item.title {
        title.push_str(" / ");
        title.push_str(extra);
    }
    if item.streaming {
        title.push_str(" ...");
    }
    lines.push(Line::styled(
        title,
        role_style(&item.role).add_modifier(Modifier::BOLD),
    ));

    if item.content.is_empty() && item.streaming {
        lines.push(Line::styled(
            "  thinking...",
            Style::default().fg(Color::DarkGray),
        ));
        return;
    }

    let mut code_block = false;
    for line in item.content.lines() {
        if line.trim_start().starts_with("```") {
            code_block = !code_block;
            lines.extend(wrap_line(
                line,
                width,
                "  ",
                Style::default().fg(Color::DarkGray),
            ));
            continue;
        }
        let style = if code_block {
            Style::default().fg(Color::Gray)
        } else {
            content_style(&item.role, line)
        };
        lines.extend(wrap_line(line, width, "  ", style));
    }
}

fn wrap_line(line: &str, width: u16, prefix: &str, style: Style) -> Vec<Line<'static>> {
    let width = width.max(1) as usize;
    let prefix_width = prefix.width();
    let available = width.saturating_sub(prefix_width).max(1);
    if line.is_empty() {
        return vec![Line::styled(prefix.to_owned(), style)];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for ch in line.chars() {
        let char_width = ch.width().unwrap_or(0);
        if !current.is_empty() && current_width + char_width > available {
            lines.push(Line::styled(format!("{prefix}{current}"), style));
            current.clear();
            current_width = 0;
        }
        current.push(ch);
        current_width += char_width;
    }
    lines.push(Line::styled(format!("{prefix}{current}"), style));
    lines
}

fn compact_render_text(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut truncated = compact
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn approval_picker_lines(state: &TuiState, width: u16) -> BuiltInput {
    let approve_selected = state.approval_selection == TuiApprovalSelection::Approve;
    let deny_selected = state.approval_selection == TuiApprovalSelection::Deny;
    let approve = approval_option_span("Approve", approve_selected, Color::Green);
    let deny = approval_option_span("Deny", deny_selected, Color::Red);
    let cursor_x = if approve_selected { 2 } else { 14 }.min(width.saturating_sub(1));
    BuiltInput {
        lines: vec![
            Line::from(vec![
                Span::styled("> ", Style::default().fg(Color::Yellow)),
                approve,
                Span::raw("  "),
                deny,
            ]),
            Line::from(vec![Span::styled(
                "  Tab/Left/Right selects, Enter confirms. You can still type a message or slash command.",
                Style::default().fg(Color::DarkGray),
            )]),
        ],
        cursor_x,
        cursor_y: 0,
    }
}

fn approval_option_span(label: &'static str, selected: bool, color: Color) -> Span<'static> {
    let text = if selected {
        format!("[{label}]")
    } else {
        format!(" {label} ")
    };
    let style = if selected {
        Style::default()
            .fg(color)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(Color::Gray)
    };
    Span::styled(text, style)
}

fn bottom_window<T>(items: Vec<T>, height: usize, scroll_from_bottom: u16) -> Vec<T> {
    if height == 0 || items.is_empty() {
        return Vec::new();
    }
    let len = items.len();
    let scroll = usize::from(scroll_from_bottom).min(len.saturating_sub(height));
    let end = len.saturating_sub(scroll);
    let start = end.saturating_sub(height);
    items.into_iter().skip(start).take(end - start).collect()
}

fn top_window<T>(items: Vec<T>, height: usize, scroll_from_top: usize) -> Vec<T> {
    if height == 0 || items.is_empty() {
        return Vec::new();
    }
    let len = items.len();
    let start = scroll_from_top.min(len.saturating_sub(height));
    let end = (start + height).min(len);
    items.into_iter().skip(start).take(end - start).collect()
}

pub(super) fn input_panel_height(state: &TuiState, area: Rect) -> u16 {
    let width = area.width.saturating_sub(2).max(1);
    let line_count = input_lines(state, width).lines.len() as u16;
    let wanted = line_count
        .saturating_add(2)
        .clamp(MIN_INPUT_HEIGHT, MAX_INPUT_HEIGHT);
    let available = area.height.saturating_sub(9);
    wanted.min(available.max(MIN_INPUT_HEIGHT))
}

fn panel_block(title: impl Into<String>) -> Block<'static> {
    panel_block_with_focus(title, false)
}

fn focused_panel_block(
    state: &TuiState,
    panel: TuiFocusPanel,
    title: impl Into<String>,
) -> Block<'static> {
    let focused = state.focused_panel == panel;
    let title = title.into();
    let title = if focused { format!("> {title}") } else { title };
    panel_block_with_focus(title, focused)
}

fn panel_block_with_focus(title: impl Into<String>, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Block::default()
        .title(title.into())
        .borders(Borders::ALL)
        .border_style(style)
}

fn busy_style(busy: bool) -> Style {
    if busy {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    }
}

fn role_style(role: &TranscriptRole) -> Style {
    match role {
        TranscriptRole::User => Style::default().fg(Color::Green),
        TranscriptRole::Assistant => Style::default().fg(Color::Cyan),
        TranscriptRole::System => Style::default().fg(Color::Yellow),
        TranscriptRole::Tool => Style::default().fg(Color::Magenta),
    }
}

fn activity_kind_style(kind: &TuiActivityKind) -> Style {
    match kind {
        TuiActivityKind::System => Style::default().fg(Color::DarkGray),
        TuiActivityKind::Chat => Style::default().fg(Color::Cyan),
        TuiActivityKind::Tool => Style::default().fg(Color::Magenta),
        TuiActivityKind::Context => Style::default().fg(Color::Green),
        TuiActivityKind::Policy => Style::default().fg(Color::Yellow),
        TuiActivityKind::Approval => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        TuiActivityKind::Run => Style::default().fg(Color::Cyan),
        TuiActivityKind::Cancellation => Style::default().fg(Color::Yellow),
        TuiActivityKind::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

fn content_style(role: &TranscriptRole, line: &str) -> Style {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return Style::default().add_modifier(Modifier::BOLD);
    }
    match role {
        TranscriptRole::System => Style::default().fg(Color::Gray),
        TranscriptRole::Tool => Style::default().fg(Color::Gray),
        _ => Style::default(),
    }
}

fn buffer_to_string(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut lines = Vec::new();
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        for x in area.left()..area.right() {
            line.push_str(buffer[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_owned());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        chat::ChatLlmOptions,
        runtime_config::ResolvedRuntimeSources,
        tools::ToolOverrides,
        tui::{
            data::{TuiAgentSummary, TuiOptions, TuiPendingApproval, TuiSelectionPoint},
            policy::TuiToolRisk,
        },
    };
    use camino::Utf8PathBuf;
    use std::{collections::VecDeque, vec};

    fn test_state() -> TuiState {
        TuiState {
            options: TuiOptions {
                runtime_sources: ResolvedRuntimeSources::new(
                    Utf8PathBuf::from("agents.yaml"),
                    None,
                ),
                trace_path: None,
                store_path: Utf8PathBuf::from("store"),
                tool_overrides: ToolOverrides::default(),
                allow_high_risk_tools: true,
                chat: ChatLlmOptions {
                    provider: "mock".to_owned(),
                    model: "mock-model".to_owned(),
                    mock_response: "ok".to_owned(),
                    api_base_url: None,
                    api_key_env: "OPENAI_API_KEY".to_owned(),
                    anthropic_version: "2023-06-01".to_owned(),
                    temperature: None,
                    max_output_tokens: None,
                    max_tool_rounds: 4,
                },
                timeout_seconds: 60,
                max_retries: 0,
                retry_backoff_ms: 0,
                hooks: Vec::new(),
                context_policy: Default::default(),
                default_agent: None,
                mouse_capture: false,
                once: false,
            },
            selected_agent_id: Some("echo_agent".to_owned()),
            agents: vec![TuiAgentSummary {
                id: "echo_agent".to_owned(),
                name: "Echo Agent".to_owned(),
            }],
            catalog_summary: None,
            trace: None,
            trace_label: None,
            recent_runs: Vec::new(),
            status: "ready".to_owned(),
            input_mode: true,
            command_input: String::new(),
            input_cursor: 0,
            transcript: Vec::new(),
            active_assistant_index: None,
            events: VecDeque::new(),
            activity: VecDeque::new(),
            tool_inventory: None,
            context_status: None,
            latest_run: None,
            latest_workflow: None,
            latest_proposals: None,
            latest_events: None,
            pending_approval: None,
            approval_selection: TuiApprovalSelection::Approve,
            chat_messages: Vec::new(),
            chat_scroll: 0,
            context_scroll: 0,
            event_scroll: 0,
            focused_panel: TuiFocusPanel::Chat,
            pane_sizing: Default::default(),
            text_selection: None,
            input_history: VecDeque::new(),
            history_cursor: None,
            history_draft: None,
            busy: false,
        }
    }

    #[test]
    fn input_lines_place_cursor_after_inserted_text() {
        let mut state = test_state();
        state.replace_command_input("hello");
        state.move_cursor_left();
        let input = input_lines(&state, 24);

        assert_eq!(input.cursor_y, 0);
        assert_eq!(input.cursor_x, 6);
    }

    #[test]
    fn input_lines_show_guided_placeholder() {
        let mut state = test_state();
        let input = input_lines(&state, 40);

        assert_eq!(input.lines[0].to_string(), "> message, /status, or /help");

        state.replace_command_input("/");
        let slash_input = input_lines(&state, 40);

        assert_eq!(slash_input.lines[0].to_string(), "/ command, Tab completes");
    }

    #[test]
    fn input_cursor_for_click_maps_plain_text_position() {
        let mut state = test_state();
        state.replace_command_input("hello");
        let area = Rect::new(0, 0, 40, 3);

        let cursor = input_cursor_for_click(&state, area, 1 + 4, 1).expect("click maps to cursor");

        assert_eq!(cursor, 3);
    }

    #[test]
    fn input_cursor_for_click_accounts_for_slash_prompt() {
        let mut state = test_state();
        state.replace_command_input("/status");
        let area = Rect::new(0, 0, 40, 3);

        let cursor = input_cursor_for_click(&state, area, 1 + 4, 1).expect("click maps to cursor");

        assert_eq!(cursor, 4);
    }

    #[test]
    fn command_panel_title_mentions_tab_completion() {
        let state = test_state();
        let rendered = render_tui_once(&state).expect("tui renders");

        assert!(rendered.contains("Tab completes"));
    }

    #[test]
    fn focused_panel_title_is_marked() {
        let mut state = test_state();
        let rendered = render_tui_once(&state).expect("tui renders");

        assert!(rendered.contains("> Chat"));

        state.focused_panel = TuiFocusPanel::Activity;
        let rendered = render_tui_once(&state).expect("tui renders");

        assert!(rendered.contains("> Activity"));
    }

    #[test]
    fn selected_text_uses_panel_local_columns() {
        let mut state = test_state();
        state.push_user_message("alpha");
        state.begin_text_selection(TuiFocusPanel::Chat, TuiSelectionPoint::new(2, 1));
        state.update_text_selection(TuiFocusPanel::Chat, TuiSelectionPoint::new(7, 1));
        let area = Rect::new(0, 0, 80, 24);
        let layout = TuiLayout::new(area, state.pane_sizing, input_panel_height(&state, area));

        let selected = selected_text_for_layout(&state, layout).expect("selection has text");

        assert_eq!(selected, "alpha");
    }

    #[test]
    fn input_panel_prioritizes_pending_approval_guidance() {
        let mut state = test_state();
        state.pending_approval = Some(TuiPendingApproval::tool_call(
            "agent.run",
            TuiToolRisk::High,
            serde_json::json!({}),
        ));

        let input = input_lines(&state, 40);
        let rendered = render_tui_once(&state).expect("tui renders");

        assert_eq!(input.lines[0].to_string(), "> [Approve]   Deny ");
        assert!(rendered.contains("pending approval: Tab selects  Enter confirms"));
        assert!(rendered.contains("Tab/Left/Right selects, Enter confirms"));
    }

    #[test]
    fn approval_picker_renders_selected_deny_option() {
        let mut state = test_state();
        state.pending_approval = Some(TuiPendingApproval::tool_call(
            "agent.run",
            TuiToolRisk::High,
            serde_json::json!({}),
        ));
        state.approval_selection = TuiApprovalSelection::Deny;

        let input = input_lines(&state, 40);

        assert_eq!(input.lines[0].to_string(), ">  Approve   [Deny]");
    }

    #[test]
    fn bottom_window_keeps_latest_items_by_default() {
        let items = vec![1, 2, 3, 4, 5];

        assert_eq!(bottom_window(items.clone(), 3, 0), vec![3, 4, 5]);
        assert_eq!(bottom_window(items, 3, 2), vec![1, 2, 3]);
    }
}
