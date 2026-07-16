use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) enum TuiContextClickAction {
    InspectProposal(String),
}

pub(in crate::tui) struct ContextPanelItem {
    pub(in crate::tui) line: Line<'static>,
    pub(in crate::tui) action: Option<TuiContextClickAction>,
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

pub(in crate::tui) fn render_tui_once(state: &TuiState) -> Result<String> {
    render_tui_at_size(state, 110, 34)
}

pub(in crate::tui) fn render_tui_at_size(
    state: &TuiState,
    width: u16,
    height: u16,
) -> Result<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).into_diagnostic()?;
    terminal
        .draw(|frame| render_tui_frame(frame, state))
        .into_diagnostic()?;
    Ok(buffer_to_string(terminal.backend().buffer()))
}

pub(in crate::tui) fn render_tui_frame(frame: &mut Frame<'_>, state: &TuiState) {
    let area = frame.area();
    let input_height = input_panel_height(state, area);
    let layout = TuiLayout::new(
        area,
        state.pane_sizing,
        input_height,
        state.focused_panel,
        state.sidebar_panel,
    );

    frame.render_widget(status_line(state), layout.status);
    if layout.chat.width > 0 {
        frame.render_widget(chat_panel(state, layout.chat), layout.chat);
        render_chat_scrollbar(frame, state, layout.chat);
    }
    if layout.context.width > 0 {
        frame.render_widget(context_panel(state, layout.context), layout.context);
        render_context_scrollbar(frame, state, layout.context);
    }
    if layout.activity.width > 0 {
        frame.render_widget(activity_panel(state, layout.activity), layout.activity);
        render_activity_scrollbar(frame, state, layout.activity);
    }

    let input = command_panel(state, layout.input);
    frame.render_widget(input.paragraph, layout.input);
    if state.pending_approval.is_none() {
        render_completion_menu(frame, state, area, layout.input);
    }
    if state.input_mode
        && state.pending_approval.is_none()
        && layout.input.width > 2
        && layout.input.height > 2
    {
        frame.set_cursor_position((
            layout.input.x + 1 + input.cursor_x,
            layout.input.y + 1 + input.cursor_y,
        ));
    }
    if state.pending_approval.is_some() {
        render_approval_overlay(frame, state, area);
    }
}

pub(in crate::tui) fn status_line(state: &TuiState) -> Paragraph<'static> {
    let status = if state.pending_approval.is_some() {
        "approval required".to_owned()
    } else if state.busy {
        state.operation_status()
    } else {
        "ready".to_owned()
    };
    Paragraph::new(Line::from(vec![
        Span::styled("Agent Runtime", theme::strong(theme::ACCENT)),
        Span::styled(
            format!("  {}", state.active_agent_label()),
            Style::default().fg(theme::TEXT),
        ),
        Span::styled(
            format!(
                "  {} / {}",
                state.options.chat.provider, state.options.chat.model
            ),
            Style::default().fg(theme::MUTED),
        ),
        Span::raw("  |  "),
        Span::styled(status, status_style(state)),
    ]))
}

pub(in crate::tui) fn context_panel(state: &TuiState, area: Rect) -> List<'static> {
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

pub(in crate::tui) fn context_action_for_click(
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

pub(in crate::tui) fn visible_context_items(state: &TuiState, area: Rect) -> Vec<ContextPanelItem> {
    let height = area.height.saturating_sub(2) as usize;
    let items = context_panel_items(state);
    let max_scroll = items.len().saturating_sub(height);
    let scroll = usize::from(state.context_scroll).min(max_scroll);
    top_window(items, height, scroll)
}

pub(in crate::tui) fn context_panel_items(state: &TuiState) -> Vec<ContextPanelItem> {
    let mut items = vec![context_section("Session")];
    items.push(context_item(format!(
        "agent  {}",
        state.active_agent_label()
    )));
    items.push(context_item(format!(
        "model  {} / {}",
        state.options.chat.provider, state.options.chat.model
    )));
    if let Some(summary) = &state.catalog_summary {
        items.push(context_item(format!(
            "catalog  {} agents / {} tools",
            summary.agent_count, summary.tool_count
        )));
    } else {
        items.push(context_item("catalog  not loaded"));
    }
    if let Some(inventory) = &state.tool_inventory {
        items.push(context_item(format!(
            "tools  {} | high {} | blocked {}",
            inventory.total_count(),
            inventory.high_risk_count(),
            inventory.blocked_count()
        )));
    }

    match state.detail_kind {
        TuiDetailKind::Overview => append_overview_items(state, &mut items),
        TuiDetailKind::Run => match &state.latest_run {
            Some(run) => items.extend(run_context_items(run)),
            None => items.extend(empty_detail_items("Run", "No run selected")),
        },
        TuiDetailKind::Workflow => match &state.latest_workflow {
            Some(workflow) => items.extend(workflow_context_items(workflow)),
            None => items.extend(empty_detail_items("Workflow", "No workflow selected")),
        },
        TuiDetailKind::Proposals => match &state.latest_proposals {
            Some(proposals) => items.extend(proposal_context_items(proposals)),
            None => items.extend(empty_detail_items("Proposals", "No proposals loaded")),
        },
        TuiDetailKind::Events => match &state.latest_events {
            Some(events) => items.extend(trace_event_context_items(events)),
            None => items.extend(empty_detail_items("Events", "No events loaded")),
        },
    }
    items
}

pub(in crate::tui) fn append_overview_items(state: &TuiState, items: &mut Vec<ContextPanelItem>) {
    if let Some(trace) = &state.trace {
        items.push(context_item(""));
        items.push(context_section("Current trace"));
        items.push(context_item(format!("run  {}", trace.run_id.0)));
        items.push(context_item(format!(
            "agent  {}@{}",
            trace.agent_id, trace.agent_version
        )));
        items.push(context_item(format!("events  {}", trace.events.len())));
    }
    if let Some(context) = &state.context_status {
        items.push(context_item(""));
        items.push(context_section("Chat context"));
        items.push(context_item(format!(
            "tokens  {}/{}",
            context.token_estimate, context.max_input_tokens
        )));
        items.push(context_item(format!("blocks  {}", context.block_count)));
        if context.compacted {
            let strategy = context.compaction_strategy.as_deref().unwrap_or("unknown");
            items.push(context_item(format!(
                "compacted  {} omitted ({strategy})",
                context.omitted_block_count
            )));
        } else {
            items.push(context_item("compacted  no"));
        }
    }
    if state.trace.is_none() && state.context_status.is_none() {
        items.extend(empty_detail_items("Overview", "No active run"));
    }
}

pub(in crate::tui) fn empty_detail_items(
    title: &'static str,
    message: &'static str,
) -> Vec<ContextPanelItem> {
    vec![
        context_item(""),
        context_section(title),
        context_item(Line::styled(message, Style::default().fg(theme::MUTED))),
    ]
}

pub(in crate::tui) fn context_section(title: impl Into<String>) -> ContextPanelItem {
    context_item(Line::styled(title.into(), theme::strong(theme::ACCENT)))
}

pub(in crate::tui) fn context_item(content: impl Into<Line<'static>>) -> ContextPanelItem {
    ContextPanelItem::inert(content.into())
}

pub(in crate::tui) fn context_panel_title(scroll: usize, max_scroll: usize) -> String {
    if max_scroll == 0 {
        "[Details]  Timeline".to_owned()
    } else if scroll == 0 {
        format!("[Details]  Timeline  +{max_scroll}")
    } else {
        format!("[Details]  Timeline  {scroll}/{max_scroll}")
    }
}

pub(in crate::tui) fn run_context_items(run: &TuiRunSummary) -> Vec<ContextPanelItem> {
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

pub(in crate::tui) fn workflow_context_items(
    workflow: &TuiWorkflowSummary,
) -> Vec<ContextPanelItem> {
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
            detail.push(' ');
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

pub(in crate::tui) fn trace_event_context_items(
    events: &TuiTraceEventSummary,
) -> Vec<ContextPanelItem> {
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

pub(in crate::tui) fn proposal_context_items(
    proposals: &TuiProposalListSummary,
) -> Vec<ContextPanelItem> {
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

pub(in crate::tui) fn visible_chat_lines(state: &TuiState, area: Rect) -> Vec<Line<'static>> {
    let width = area.width.saturating_sub(2).max(1);
    let height = area.height.saturating_sub(2) as usize;
    bottom_window(chat_lines(state, width), height, state.chat_scroll)
}

pub(in crate::tui) fn chat_panel(state: &TuiState, area: Rect) -> Paragraph<'static> {
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

pub(in crate::tui) fn activity_panel(state: &TuiState, area: Rect) -> List<'static> {
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
        "Details  [Timeline]".to_owned()
    } else {
        format!("Details  [Timeline]  +{}", state.event_scroll)
    };
    List::new(visible).block(focused_panel_block(state, TuiFocusPanel::Activity, title))
}

pub(in crate::tui) fn visible_activity_lines(state: &TuiState, area: Rect) -> Vec<Line<'static>> {
    let items = activity_lines(state);
    let height = area.height.saturating_sub(2) as usize;
    bottom_window(items, height, state.event_scroll)
}

pub(in crate::tui) fn activity_lines(state: &TuiState) -> Vec<Line<'static>> {
    let mut items = Vec::new();
    if state.activity.is_empty() {
        items.push(Line::styled(
            "No activity yet",
            Style::default().fg(theme::MUTED),
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

    items
}

pub(in crate::tui) fn activity_item_line(activity: &TuiActivityItem) -> Line<'static> {
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

pub(in crate::tui) fn render_chat_scrollbar(frame: &mut Frame<'_>, state: &TuiState, area: Rect) {
    let total = chat_lines(state, area.width.saturating_sub(2).max(1)).len();
    render_bottom_scrollbar(frame, area, total, state.chat_scroll);
}

pub(in crate::tui) fn render_context_scrollbar(
    frame: &mut Frame<'_>,
    state: &TuiState,
    area: Rect,
) {
    let total = context_panel_items(state).len();
    render_top_scrollbar(frame, area, total, state.context_scroll);
}

pub(in crate::tui) fn render_activity_scrollbar(
    frame: &mut Frame<'_>,
    state: &TuiState,
    area: Rect,
) {
    let total = activity_lines(state).len();
    render_bottom_scrollbar(frame, area, total, state.event_scroll);
}

pub(in crate::tui) fn render_bottom_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    total: usize,
    offset: u16,
) {
    let visible = area.height.saturating_sub(2) as usize;
    let max_position = total.saturating_sub(visible);
    let position = max_position.saturating_sub(usize::from(offset).min(max_position));
    render_scrollbar(frame, area, total, visible, position);
}

pub(in crate::tui) fn render_top_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    total: usize,
    offset: u16,
) {
    let visible = area.height.saturating_sub(2) as usize;
    let position = usize::from(offset).min(total.saturating_sub(visible));
    render_scrollbar(frame, area, total, visible, position);
}

pub(in crate::tui) fn render_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    total: usize,
    visible: usize,
    position: usize,
) {
    if total <= visible || visible == 0 {
        return;
    }
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .thumb_symbol("┃");
    let mut scrollbar_state = ScrollbarState::new(total)
        .position(position)
        .viewport_content_length(visible);
    frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
}
