use super::*;

pub(in crate::tui) struct InputPanel {
    pub(in crate::tui) paragraph: Paragraph<'static>,
    pub(in crate::tui) cursor_x: u16,
    pub(in crate::tui) cursor_y: u16,
}

pub(in crate::tui) fn command_panel(state: &TuiState, area: Rect) -> InputPanel {
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
    let title = if state.pending_approval.is_some() {
        "Approval pending"
    } else if state.command_input.starts_with('/') {
        "Command"
    } else if state.busy {
        "Running"
    } else {
        "Message"
    };

    InputPanel {
        paragraph: Paragraph::new(Text::from(lines)).block(panel_block_with_focus(
            title,
            state.input_mode && state.pending_approval.is_none(),
        )),
        cursor_x: built.cursor_x.min(inner_width.saturating_sub(1)),
        cursor_y: cursor_y.min(area.height.saturating_sub(3)),
    }
}

pub(in crate::tui) fn render_completion_menu(
    frame: &mut Frame<'_>,
    state: &TuiState,
    area: Rect,
    input: Rect,
) {
    let Some(menu) = &state.completion else {
        return;
    };
    let Some(menu_area) = completion_menu_area(state, area, input) else {
        return;
    };
    let visible_count = menu_area.height.saturating_sub(2) as usize;
    let start = menu
        .selected
        .saturating_sub(visible_count.saturating_sub(1));
    let items = menu
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_count)
        .map(|(index, item)| {
            let style = if index == menu.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT)
            };
            let line = Line::from(vec![
                Span::styled(item.label.clone(), style),
                Span::styled(
                    item.description
                        .as_ref()
                        .map(|description| format!("  {description}"))
                        .unwrap_or_default(),
                    if index == menu.selected {
                        style
                    } else {
                        Style::default().fg(theme::MUTED)
                    },
                ),
            ]);
            ListItem::new(line)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Clear, menu_area);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(format!(" {} ", menu.title))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(theme::panel(true)),
        ),
        menu_area,
    );
}

pub(in crate::tui) fn completion_menu_area(
    state: &TuiState,
    area: Rect,
    input: Rect,
) -> Option<Rect> {
    let menu = state.completion.as_ref()?;
    let available_height = input.y.saturating_sub(area.y);
    if available_height < 3 {
        return None;
    }
    let height = (menu.items.len() as u16 + 2).min(9).min(available_height);
    let content_width = menu
        .items
        .iter()
        .map(|item| {
            (item.label.width()
                + item
                    .description
                    .as_ref()
                    .map(|description| description.width() + 2)
                    .unwrap_or_default()) as u16
        })
        .max()
        .unwrap_or(12)
        .saturating_add(4);
    let margin = if input.width >= 6 { 2 } else { 0 };
    let width = content_width
        .clamp(24, 52)
        .min(input.width.saturating_sub(margin * 2).max(1));
    Some(Rect::new(
        input.x.saturating_add(margin),
        input.y.saturating_sub(height),
        width,
        height,
    ))
}

pub(in crate::tui) fn completion_index_at_position(
    state: &TuiState,
    area: Rect,
    input: Rect,
    column: u16,
    row: u16,
) -> Option<usize> {
    let menu = state.completion.as_ref()?;
    let menu_area = completion_menu_area(state, area, input)?;
    if column <= menu_area.x
        || column
            >= menu_area
                .x
                .saturating_add(menu_area.width)
                .saturating_sub(1)
        || row <= menu_area.y
        || row
            >= menu_area
                .y
                .saturating_add(menu_area.height)
                .saturating_sub(1)
    {
        return None;
    }
    let visible_count = menu_area.height.saturating_sub(2) as usize;
    let start = menu
        .selected
        .saturating_sub(visible_count.saturating_sub(1));
    let index = start + usize::from(row.saturating_sub(menu_area.y + 1));
    (index < menu.items.len()).then_some(index)
}

pub(in crate::tui) struct BuiltInput {
    pub(in crate::tui) lines: Vec<Line<'static>>,
    pub(in crate::tui) cursor_x: u16,
    pub(in crate::tui) cursor_y: usize,
}

pub(in crate::tui) fn input_cursor_for_click(
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

pub(in crate::tui) fn input_lines(state: &TuiState, width: u16) -> BuiltInput {
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
        let placeholder = if slash {
            "command"
        } else if state.pending_approval.is_some() {
            "resolve the pending request"
        } else {
            "message or /command"
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
pub(in crate::tui) struct InputClickRow {
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
pub(in crate::tui) struct InputClickSegment {
    start_col: u16,
    end_col: u16,
    before_cursor: usize,
    after_cursor: usize,
}

pub(in crate::tui) fn input_click_rows(state: &TuiState, width: u16) -> Vec<InputClickRow> {
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

pub(in crate::tui) fn push_input_line(
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
