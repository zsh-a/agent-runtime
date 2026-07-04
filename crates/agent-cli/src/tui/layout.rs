use ratatui::layout::Rect;

use super::data::{TuiFocusPanel, TuiPaneSizing};

const DEFAULT_CHAT_PERCENT: u16 = 72;
const DEFAULT_CONTEXT_HEIGHT: u16 = 15;
const MIN_CHAT_WIDTH: u16 = 30;
const MIN_SIDE_WIDTH: u16 = 22;
const MIN_CONTEXT_HEIGHT: u16 = 5;
const MIN_ACTIVITY_HEIGHT: u16 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TuiLayout {
    pub(super) status: Rect,
    pub(super) chat: Rect,
    pub(super) context: Rect,
    pub(super) activity: Rect,
    pub(super) input: Rect,
}

impl TuiLayout {
    pub(super) fn new(area: Rect, sizing: TuiPaneSizing, input_height: u16) -> Self {
        let input_height = input_height.min(area.height);
        let status_height = area.height.min(1);
        let input_y = area
            .y
            .saturating_add(area.height)
            .saturating_sub(input_height);
        let body_y = area.y.saturating_add(status_height);
        let body_height = input_y.saturating_sub(body_y);

        let requested_chat_width = sizing
            .side_width
            .map(|side_width| area.width.saturating_sub(side_width))
            .unwrap_or_else(|| area.width.saturating_mul(DEFAULT_CHAT_PERCENT) / 100);
        let chat_width = clamp_split(
            area.width,
            requested_chat_width,
            MIN_CHAT_WIDTH,
            MIN_SIDE_WIDTH,
        );
        let side_width = area.width.saturating_sub(chat_width);

        let requested_context_height = sizing.context_height.unwrap_or(DEFAULT_CONTEXT_HEIGHT);
        let context_height = clamp_split(
            body_height,
            requested_context_height,
            MIN_CONTEXT_HEIGHT,
            MIN_ACTIVITY_HEIGHT,
        );
        let activity_height = body_height.saturating_sub(context_height);

        let side_x = area.x.saturating_add(chat_width);
        let activity_y = body_y.saturating_add(context_height);

        Self {
            status: Rect::new(area.x, area.y, area.width, status_height),
            chat: Rect::new(area.x, body_y, chat_width, body_height),
            context: Rect::new(side_x, body_y, side_width, context_height),
            activity: Rect::new(side_x, activity_y, side_width, activity_height),
            input: Rect::new(area.x, input_y, area.width, input_height),
        }
    }

    pub(super) fn focus_panel_at(self, column: u16, row: u16) -> Option<TuiFocusPanel> {
        if contains(self.chat, column, row) {
            Some(TuiFocusPanel::Chat)
        } else if contains(self.context, column, row) {
            Some(TuiFocusPanel::Context)
        } else if contains(self.activity, column, row) {
            Some(TuiFocusPanel::Activity)
        } else {
            None
        }
    }

    pub(super) fn resize_handle_at(self, column: u16, row: u16) -> Option<TuiResizeHandle> {
        if !contains(self.body(), column, row) {
            return None;
        }
        let side_left = self.context.x;
        let chat_right = self
            .chat
            .x
            .saturating_add(self.chat.width)
            .saturating_sub(1);
        if column == side_left || column == chat_right {
            return Some(TuiResizeHandle::Side);
        }

        let context_bottom = self
            .context
            .y
            .saturating_add(self.context.height)
            .saturating_sub(1);
        let activity_top = self.activity.y;
        if column >= self.context.x
            && column < self.context.x.saturating_add(self.context.width)
            && (row == context_bottom || row == activity_top)
        {
            return Some(TuiResizeHandle::ContextActivity);
        }

        None
    }

    pub(super) fn resize(self, handle: TuiResizeHandle, column: u16, row: u16) -> TuiPaneSizing {
        match handle {
            TuiResizeHandle::Side => {
                let requested_chat_width = column.saturating_sub(self.chat.x).max(1);
                let chat_width = clamp_split(
                    self.chat.width.saturating_add(self.context.width),
                    requested_chat_width,
                    MIN_CHAT_WIDTH,
                    MIN_SIDE_WIDTH,
                );
                TuiPaneSizing {
                    side_width: Some(
                        self.chat
                            .width
                            .saturating_add(self.context.width)
                            .saturating_sub(chat_width),
                    ),
                    ..TuiPaneSizing::default()
                }
            }
            TuiResizeHandle::ContextActivity => {
                let body_height = self.context.height.saturating_add(self.activity.height);
                let requested_context_height = row.saturating_sub(self.context.y).max(1);
                let context_height = clamp_split(
                    body_height,
                    requested_context_height,
                    MIN_CONTEXT_HEIGHT,
                    MIN_ACTIVITY_HEIGHT,
                );
                TuiPaneSizing {
                    context_height: Some(context_height),
                    ..TuiPaneSizing::default()
                }
            }
        }
    }

    fn body(self) -> Rect {
        Rect::new(
            self.chat.x,
            self.chat.y,
            self.chat.width.saturating_add(self.context.width),
            self.chat.height,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TuiResizeHandle {
    Side,
    ContextActivity,
}

pub(super) fn contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

fn clamp_split(total: u16, requested_first: u16, min_first: u16, min_second: u16) -> u16 {
    if total <= 1 {
        return total;
    }
    if total < min_first.saturating_add(min_second) {
        return requested_first.clamp(1, total.saturating_sub(1));
    }
    requested_first.clamp(min_first, total.saturating_sub(min_second))
}
