use super::*;

pub(in crate::tui) fn selected_text_for_layout(
    state: &TuiState,
    layout: TuiLayout,
) -> Option<String> {
    let selection = state.text_selection.as_ref()?;
    if selection.is_empty() {
        return None;
    }
    let lines = visible_text_lines_for_panel(state, layout, selection.panel);
    selected_text_from_lines(&lines, selection)
}

pub(in crate::tui) fn visible_text_lines_for_panel(
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

pub(in crate::tui) fn highlight_selection_line(
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

pub(in crate::tui) fn selected_text_from_lines(
    lines: &[String],
    selection: &TuiTextSelection,
) -> Option<String> {
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

pub(in crate::tui) fn selected_columns_for_row(
    selection: &TuiTextSelection,
    row: usize,
) -> Option<(u16, u16)> {
    let (start, end) = selection.ordered_points();
    let row = u16::try_from(row).ok()?;
    if row < start.row || row > end.row {
        return None;
    }
    let start_col = if row == start.row { start.column } else { 0 };
    let end_col = if row == end.row { end.column } else { u16::MAX };
    (end_col > start_col).then_some((start_col, end_col))
}

pub(in crate::tui) fn split_display_columns(
    text: &str,
    start_col: u16,
    end_col: u16,
) -> (String, String, String) {
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

pub(in crate::tui) fn selection_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::Cyan)
}
