use super::*;

pub(in crate::tui) fn chat_lines(state: &TuiState, width: u16) -> Vec<Line<'static>> {
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

pub(in crate::tui) fn push_transcript_item(
    lines: &mut Vec<Line<'static>>,
    item: &TranscriptItem,
    width: u16,
) {
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

pub(in crate::tui) fn wrap_line(
    line: &str,
    width: u16,
    prefix: &str,
    style: Style,
) -> Vec<Line<'static>> {
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

pub(in crate::tui) fn compact_render_text(text: &str, max_chars: usize) -> String {
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
