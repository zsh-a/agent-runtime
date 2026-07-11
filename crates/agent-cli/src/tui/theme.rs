use ratatui::style::{Color, Modifier, Style};

pub(super) const ACCENT: Color = Color::Cyan;
pub(super) const MUTED: Color = Color::DarkGray;
pub(super) const TEXT: Color = Color::Gray;
pub(super) const SUCCESS: Color = Color::Green;
pub(super) const WARNING: Color = Color::Yellow;
pub(super) const DANGER: Color = Color::Red;
pub(super) fn strong(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

pub(super) fn panel(focused: bool) -> Style {
    Style::default().fg(if focused { ACCENT } else { MUTED })
}
