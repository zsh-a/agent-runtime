use ratatui::layout::Rect;

use super::data::{TuiFocusPanel, TuiPaneSizing};

const WIDE_BREAKPOINT: u16 = 90;
const DEFAULT_CHAT_PERCENT: u16 = 70;
const MIN_CHAT_WIDTH: u16 = 48;
const MIN_SIDE_WIDTH: u16 = 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TuiLayoutMode {
    Wide,
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TuiLayout {
    pub(super) mode: TuiLayoutMode,
    pub(super) status: Rect,
    pub(super) chat: Rect,
    pub(super) context: Rect,
    pub(super) activity: Rect,
    pub(super) input: Rect,
}

impl TuiLayout {
    pub(super) fn new(
        area: Rect,
        sizing: TuiPaneSizing,
        input_height: u16,
        focused_panel: TuiFocusPanel,
        sidebar_panel: TuiFocusPanel,
    ) -> Self {
        let input_height = input_height.min(area.height);
        let status_height = area.height.min(1);
        let input_y = area
            .y
            .saturating_add(area.height)
            .saturating_sub(input_height);
        let body = Rect::new(
            area.x,
            area.y.saturating_add(status_height),
            area.width,
            input_y.saturating_sub(area.y.saturating_add(status_height)),
        );
        let mode = if area.width >= WIDE_BREAKPOINT {
            TuiLayoutMode::Wide
        } else {
            TuiLayoutMode::Compact
        };
        let mut layout = Self {
            mode,
            status: Rect::new(area.x, area.y, area.width, status_height),
            chat: Rect::default(),
            context: Rect::default(),
            activity: Rect::default(),
            input: Rect::new(area.x, input_y, area.width, input_height),
        };

        match mode {
            TuiLayoutMode::Wide => {
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
                let side = Rect::new(
                    area.x.saturating_add(chat_width),
                    body.y,
                    area.width.saturating_sub(chat_width),
                    body.height,
                );
                layout.chat = Rect::new(body.x, body.y, chat_width, body.height);
                layout.set_panel(sidebar_panel, side);
            }
            TuiLayoutMode::Compact => layout.set_panel(focused_panel, body),
        }
        layout
    }

    pub(super) fn focus_panel_at(self, column: u16, row: u16) -> Option<TuiFocusPanel> {
        [
            (TuiFocusPanel::Chat, self.chat),
            (TuiFocusPanel::Context, self.context),
            (TuiFocusPanel::Activity, self.activity),
        ]
        .into_iter()
        .find_map(|(panel, rect)| contains(rect, column, row).then_some(panel))
    }

    pub(super) fn resize_handle_at(self, column: u16, row: u16) -> Option<TuiResizeHandle> {
        if self.mode != TuiLayoutMode::Wide || !contains(self.body(), column, row) {
            return None;
        }
        let side = self.side_panel();
        let chat_right = self
            .chat
            .x
            .saturating_add(self.chat.width)
            .saturating_sub(1);
        (column == side.x || column == chat_right).then_some(TuiResizeHandle::Side)
    }

    pub(super) fn resize(self, _: TuiResizeHandle, column: u16, _: u16) -> TuiPaneSizing {
        let total_width = self.chat.width.saturating_add(self.side_panel().width);
        let requested_chat_width = column.saturating_sub(self.chat.x).max(1);
        let chat_width = clamp_split(
            total_width,
            requested_chat_width,
            MIN_CHAT_WIDTH,
            MIN_SIDE_WIDTH,
        );
        TuiPaneSizing {
            side_width: Some(total_width.saturating_sub(chat_width)),
        }
    }

    fn set_panel(&mut self, panel: TuiFocusPanel, area: Rect) {
        match panel {
            TuiFocusPanel::Chat => self.chat = area,
            TuiFocusPanel::Context => self.context = area,
            TuiFocusPanel::Activity => self.activity = area,
        }
    }

    fn side_panel(self) -> Rect {
        if self.context.width > 0 {
            self.context
        } else {
            self.activity
        }
    }

    fn body(self) -> Rect {
        let side = self.side_panel();
        Rect::new(
            self.chat.x,
            self.chat.y,
            self.chat.width.saturating_add(side.width),
            self.chat.height.max(side.height),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TuiResizeHandle {
    Side,
}

pub(super) fn contains(rect: Rect, column: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && column >= rect.x
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
