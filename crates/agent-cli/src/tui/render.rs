use miette::{IntoDiagnostic, Result};
use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use super::data::{TranscriptRole, TuiState};

pub(super) fn render_tui_once(state: &TuiState) -> Result<String> {
    let backend = TestBackend::new(110, 34);
    let mut terminal = Terminal::new(backend).into_diagnostic()?;
    terminal
        .draw(|frame| render_tui_frame(frame, state))
        .into_diagnostic()?;
    Ok(buffer_to_string(terminal.backend().buffer()))
}

pub(super) fn render_tui_frame(frame: &mut Frame<'_>, state: &TuiState) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(frame.area());
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
        .split(root[1]);
    let side = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(11), Constraint::Min(8)])
        .split(body[1]);

    frame.render_widget(status_line(state), root[0]);
    frame.render_widget(chat_panel(state), body[0]);
    frame.render_widget(context_panel(state), side[0]);
    frame.render_widget(activity_panel(state), side[1]);
    frame.render_widget(command_panel(state), root[2]);
}

fn status_line(state: &TuiState) -> Paragraph<'static> {
    let status = if state.busy { " running" } else { " ready" };
    Paragraph::new(Line::from(vec![
        Span::styled(
            "Agent Runtime",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  {} / {}  |{}  |  Enter sends  |  wheel scrolls  |  Esc exits",
            state.options.chat.provider, state.options.chat.model, status
        )),
    ]))
}

fn context_panel(state: &TuiState) -> List<'static> {
    let mut items = Vec::new();
    if let Some(summary) = &state.catalog_summary {
        items.extend([
            ListItem::new(format!(
                "catalog {} agents / {} tools",
                summary.agent_count, summary.tool_count
            )),
            ListItem::new(format!("domains {}", summary.active_domains.join(", "))),
        ]);
    } else {
        items.push(ListItem::new("catalog: not loaded"));
    }
    items.push(ListItem::new(format!(
        "trace: {}",
        state
            .trace_label
            .clone()
            .unwrap_or_else(|| "not loaded".to_owned())
    )));
    if let Some(trace) = &state.trace {
        items.extend([
            ListItem::new(format!("run {}", trace.run_id.0)),
            ListItem::new(format!("agent {}@{}", trace.agent_id, trace.agent_version)),
            ListItem::new(format!("events: {}", trace.events.len())),
        ]);
    }
    List::new(items).block(Block::default().title("Context").borders(Borders::ALL))
}

fn chat_panel(state: &TuiState) -> Paragraph<'static> {
    let mut lines = Vec::new();
    if state.transcript.is_empty() {
        lines.push(Line::from(Span::styled(
            "No messages yet.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (index, item) in state.transcript.iter().enumerate() {
            if index > 0 {
                lines.push(Line::from(""));
            }
            let mut title = item.role.label().to_owned();
            if let Some(extra) = &item.title {
                title.push_str(" / ");
                title.push_str(extra);
            }
            if item.streaming {
                title.push_str(" ...");
            }
            lines.push(Line::from(Span::styled(
                title,
                role_style(&item.role).add_modifier(Modifier::BOLD),
            )));
            if item.content.is_empty() && item.streaming {
                lines.push(Line::from(Span::styled(
                    "thinking...",
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                for line in item.content.lines() {
                    lines.push(Line::from(line.to_owned()));
                }
            }
        }
    }
    Paragraph::new(Text::from(lines))
        .block(Block::default().title("Chat").borders(Borders::ALL))
        .wrap(Wrap { trim: false })
        .scroll((state.chat_scroll, 0))
}

fn activity_panel(state: &TuiState) -> List<'static> {
    let mut items = Vec::new();
    if state.events.is_empty() {
        items.push(ListItem::new("no activity"));
    } else {
        items.extend(
            state
                .events
                .iter()
                .skip(state.event_scroll as usize)
                .map(|line| ListItem::new(line.clone())),
        );
    }
    if !state.recent_runs.is_empty() {
        items.push(ListItem::new(""));
        items.push(ListItem::new("recent runs"));
        items.extend(state.recent_runs.iter().take(4).map(|run| {
            ListItem::new(format!(
                "{} {} {:?}",
                run.run_id.0, run.agent_id, run.status
            ))
        }));
    }
    List::new(items).block(Block::default().title("Activity").borders(Borders::ALL))
}

fn command_panel(state: &TuiState) -> Paragraph<'static> {
    let prompt = if state.command_input.starts_with('/') {
        "/"
    } else {
        ">"
    };
    let text = if state.input_mode {
        if let Some(command) = state.command_input.strip_prefix('/') {
            format!("{prompt} {command}")
        } else {
            format!("{prompt} {}", state.command_input)
        }
    } else {
        "Type a message, or use /help".to_owned()
    };
    Paragraph::new(text)
        .block(Block::default().title("Input").borders(Borders::ALL))
        .wrap(Wrap { trim: false })
}

fn role_style(role: &TranscriptRole) -> Style {
    match role {
        TranscriptRole::User => Style::default().fg(Color::Green),
        TranscriptRole::Assistant => Style::default().fg(Color::Cyan),
        TranscriptRole::System => Style::default().fg(Color::Yellow),
        TranscriptRole::Tool => Style::default().fg(Color::Magenta),
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
