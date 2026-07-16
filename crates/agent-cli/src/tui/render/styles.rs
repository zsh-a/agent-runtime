use super::*;

pub(in crate::tui) fn bottom_window<T>(
    items: Vec<T>,
    height: usize,
    scroll_from_bottom: u16,
) -> Vec<T> {
    if height == 0 || items.is_empty() {
        return Vec::new();
    }
    let len = items.len();
    let scroll = usize::from(scroll_from_bottom).min(len.saturating_sub(height));
    let end = len.saturating_sub(scroll);
    let start = end.saturating_sub(height);
    items.into_iter().skip(start).take(end - start).collect()
}

pub(in crate::tui) fn top_window<T>(
    items: Vec<T>,
    height: usize,
    scroll_from_top: usize,
) -> Vec<T> {
    if height == 0 || items.is_empty() {
        return Vec::new();
    }
    let len = items.len();
    let start = scroll_from_top.min(len.saturating_sub(height));
    let end = (start + height).min(len);
    items.into_iter().skip(start).take(end - start).collect()
}

pub(in crate::tui) fn input_panel_height(state: &TuiState, area: Rect) -> u16 {
    let width = area.width.saturating_sub(2).max(1);
    let line_count = input_lines(state, width).lines.len() as u16;
    let wanted = line_count
        .saturating_add(2)
        .clamp(MIN_INPUT_HEIGHT, MAX_INPUT_HEIGHT);
    let available = area.height.saturating_sub(9);
    wanted.min(available.max(MIN_INPUT_HEIGHT))
}

pub(in crate::tui) fn focused_panel_block(
    state: &TuiState,
    panel: TuiFocusPanel,
    title: impl Into<String>,
) -> Block<'static> {
    let focused = !state.input_mode && state.focused_panel == panel;
    let title = title.into();
    let title = if focused { format!("> {title}") } else { title };
    panel_block_with_focus(title, focused)
}

pub(in crate::tui) fn panel_block_with_focus(
    title: impl Into<String>,
    focused: bool,
) -> Block<'static> {
    Block::default()
        .title(title.into())
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::panel(focused))
}

pub(in crate::tui) fn status_style(state: &TuiState) -> Style {
    if state.pending_approval.is_some() || state.busy {
        theme::strong(theme::WARNING)
    } else {
        Style::default().fg(theme::SUCCESS)
    }
}

pub(in crate::tui) fn role_style(role: &TranscriptRole) -> Style {
    match role {
        TranscriptRole::User => Style::default().fg(Color::Green),
        TranscriptRole::Assistant => Style::default().fg(Color::Cyan),
        TranscriptRole::System => Style::default().fg(Color::Yellow),
        TranscriptRole::Tool => Style::default().fg(Color::Magenta),
    }
}

pub(in crate::tui) fn activity_kind_style(kind: &TuiActivityKind) -> Style {
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

pub(in crate::tui) fn content_style(role: &TranscriptRole, line: &str) -> Style {
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

pub(in crate::tui) fn buffer_to_string(buffer: &Buffer) -> String {
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
