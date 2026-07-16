use super::*;

impl TuiState {
    pub(in crate::tui) fn scroll_chat_up(&mut self) {
        self.chat_scroll = self.chat_scroll.saturating_add(SCROLL_LINES);
    }

    pub(in crate::tui) fn scroll_chat_down(&mut self) {
        self.chat_scroll = self.chat_scroll.saturating_sub(SCROLL_LINES);
    }

    pub(in crate::tui) fn scroll_activity_up(&mut self) {
        self.event_scroll = self.event_scroll.saturating_add(SCROLL_LINES);
    }

    pub(in crate::tui) fn scroll_activity_down(&mut self) {
        self.event_scroll = self.event_scroll.saturating_sub(SCROLL_LINES);
    }

    pub(in crate::tui) fn scroll_context_up(&mut self) {
        self.context_scroll = self.context_scroll.saturating_sub(SCROLL_LINES);
    }

    pub(in crate::tui) fn scroll_context_down(&mut self) {
        self.context_scroll = self.context_scroll.saturating_add(SCROLL_LINES);
    }

    pub(in crate::tui) fn focus_panel(&mut self, panel: TuiFocusPanel) {
        self.focused_panel = panel;
        if panel != TuiFocusPanel::Chat {
            self.sidebar_panel = panel;
        }
    }

    pub(in crate::tui) fn focus_next_panel(&mut self) {
        self.input_mode = false;
        self.focus_panel(self.focused_panel.next());
    }

    pub(in crate::tui) fn focus_previous_panel(&mut self) {
        self.input_mode = false;
        self.focus_panel(self.focused_panel.previous());
    }

    pub(in crate::tui) fn enter_input_mode(&mut self) {
        self.input_mode = true;
    }

    pub(in crate::tui) fn leave_input_mode(&mut self) {
        self.input_mode = false;
    }

    pub(in crate::tui) fn begin_text_selection(
        &mut self,
        panel: TuiFocusPanel,
        point: TuiSelectionPoint,
    ) {
        self.text_selection = Some(TuiTextSelection::new(panel, point));
    }

    pub(in crate::tui) fn update_text_selection(
        &mut self,
        panel: TuiFocusPanel,
        point: TuiSelectionPoint,
    ) {
        match self.text_selection.as_mut() {
            Some(selection) if selection.panel == panel => selection.update(point),
            _ => self.begin_text_selection(panel, point),
        }
    }

    pub(in crate::tui) fn finish_text_selection(&mut self) -> bool {
        let Some(selection) = self.text_selection.as_ref() else {
            return false;
        };
        !selection.is_empty()
    }

    pub(in crate::tui) fn clear_text_selection(&mut self) {
        self.text_selection = None;
    }

    pub(in crate::tui) fn scroll_focused_panel_up(&mut self) {
        match self.focused_panel {
            TuiFocusPanel::Chat => self.scroll_chat_up(),
            TuiFocusPanel::Context => self.scroll_context_up(),
            TuiFocusPanel::Activity => self.scroll_activity_up(),
        }
    }

    pub(in crate::tui) fn scroll_focused_panel_down(&mut self) {
        match self.focused_panel {
            TuiFocusPanel::Chat => self.scroll_chat_down(),
            TuiFocusPanel::Context => self.scroll_context_down(),
            TuiFocusPanel::Activity => self.scroll_activity_down(),
        }
    }

    pub(in crate::tui) fn scroll_chat_top(&mut self) {
        self.chat_scroll = u16::MAX / 2;
    }

    pub(in crate::tui) fn scroll_chat_bottom(&mut self) {
        self.chat_scroll = 0;
    }
}
