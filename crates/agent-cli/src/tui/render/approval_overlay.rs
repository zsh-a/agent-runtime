use super::*;

pub(in crate::tui) fn approval_option_span(
    label: &'static str,
    selected: bool,
    color: Color,
) -> Span<'static> {
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

pub(in crate::tui) fn render_approval_overlay(frame: &mut Frame<'_>, state: &TuiState, area: Rect) {
    let Some(approval) = &state.pending_approval else {
        return;
    };
    let modal = approval_modal_area(area);
    frame.render_widget(Clear, modal);

    let approve_selected = state.approval_selection == Some(TuiApprovalSelection::Approve);
    let deny_selected = state.approval_selection == Some(TuiApprovalSelection::Deny);
    let input = match &approval.action {
        TuiPendingApprovalAction::SlashTool { input, .. } => {
            serde_json::to_string(input).unwrap_or_else(|_| "<unavailable>".to_owned())
        }
        TuiPendingApprovalAction::ChatTools { tool_calls, .. } => tool_calls
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    };
    let text = Text::from(vec![
        Line::from(vec![
            Span::styled("Tool  ", Style::default().fg(theme::MUTED)),
            Span::styled(approval.subject(), theme::strong(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::styled("Risk  ", Style::default().fg(theme::MUTED)),
            Span::styled(approval.risk.label(), theme::strong(theme::WARNING)),
        ]),
        Line::from(""),
        Line::styled(
            compact_render_text(&input, 180),
            Style::default().fg(theme::TEXT),
        ),
        Line::from(""),
        Line::from(vec![
            approval_option_span("Approve", approve_selected, theme::SUCCESS),
            Span::raw("    "),
            approval_option_span("Deny", deny_selected, theme::DANGER),
        ])
        .alignment(Alignment::Center),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .title(" Approval required ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(theme::strong(theme::WARNING)),
            )
            .wrap(Wrap { trim: true }),
        modal,
    );
}

pub(in crate::tui) fn approval_modal_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).clamp(1, 64);
    let height = area.height.saturating_sub(2).clamp(1, 11);
    Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    )
}

pub(in crate::tui) fn approval_selection_at_position(
    area: Rect,
    column: u16,
    row: u16,
) -> Option<TuiApprovalSelection> {
    let modal = approval_modal_area(area);
    let button_row = modal.y.saturating_add(modal.height).saturating_sub(3);
    if row != button_row {
        return None;
    }
    let center = modal.x.saturating_add(modal.width / 2);
    if column >= center.saturating_sub(14) && column < center.saturating_sub(3) {
        Some(TuiApprovalSelection::Approve)
    } else if column >= center.saturating_add(2) && column < center.saturating_add(11) {
        Some(TuiApprovalSelection::Deny)
    } else {
        None
    }
}
