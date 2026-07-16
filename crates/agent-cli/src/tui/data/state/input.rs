use super::*;

impl TuiState {
    pub(in crate::tui) fn remember_input(&mut self, input: impl Into<String>) {
        let input = input.into();
        if input.trim().is_empty() {
            return;
        }
        if self.input_history.back() == Some(&input) {
            self.history_cursor = None;
            return;
        }
        self.input_history.push_back(input);
        while self.input_history.len() > MAX_HISTORY_ITEMS {
            self.input_history.pop_front();
        }
        self.history_cursor = None;
        self.history_draft = None;
    }

    pub(in crate::tui) fn replace_command_input(&mut self, input: impl Into<String>) {
        self.completion = None;
        self.command_input = input.into();
        self.input_cursor = self.command_input.len();
        self.history_cursor = None;
        self.history_draft = None;
    }

    pub(in crate::tui) fn clear_command_input(&mut self) {
        self.completion = None;
        self.command_input.clear();
        self.input_cursor = 0;
        self.history_cursor = None;
        self.history_draft = None;
    }

    pub(in crate::tui) fn take_submitted_input(&mut self) -> String {
        let input = self.command_input.trim().to_owned();
        self.clear_command_input();
        input
    }

    pub(in crate::tui) fn insert_char(&mut self, ch: char) {
        self.break_history_navigation();
        self.command_input.insert(self.input_cursor, ch);
        self.input_cursor += ch.len_utf8();
    }

    pub(in crate::tui) fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub(in crate::tui) fn backspace(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        self.break_history_navigation();
        let previous = self.previous_char_boundary(self.input_cursor);
        self.command_input.drain(previous..self.input_cursor);
        self.input_cursor = previous;
    }

    pub(in crate::tui) fn delete(&mut self) {
        if self.input_cursor >= self.command_input.len() {
            return;
        }
        self.break_history_navigation();
        let next = self.next_char_boundary(self.input_cursor);
        self.command_input.drain(self.input_cursor..next);
    }

    pub(in crate::tui) fn delete_before_cursor(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        self.break_history_navigation();
        self.command_input.drain(..self.input_cursor);
        self.input_cursor = 0;
    }

    pub(in crate::tui) fn delete_after_cursor(&mut self) {
        if self.input_cursor >= self.command_input.len() {
            return;
        }
        self.break_history_navigation();
        self.command_input.drain(self.input_cursor..);
    }

    pub(in crate::tui) fn delete_previous_word(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        self.break_history_navigation();
        let start = self.previous_word_boundary(self.input_cursor);
        self.command_input.drain(start..self.input_cursor);
        self.input_cursor = start;
    }

    pub(in crate::tui) fn move_cursor_left(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor = self.previous_char_boundary(self.input_cursor);
        }
    }

    pub(in crate::tui) fn move_cursor_right(&mut self) {
        if self.input_cursor < self.command_input.len() {
            self.input_cursor = self.next_char_boundary(self.input_cursor);
        }
    }

    pub(in crate::tui) fn move_cursor_word_left(&mut self) {
        self.input_cursor = self.previous_word_boundary(self.input_cursor);
    }

    pub(in crate::tui) fn move_cursor_word_right(&mut self) {
        self.input_cursor = self.next_word_boundary(self.input_cursor);
    }

    pub(in crate::tui) fn move_cursor_to_start(&mut self) {
        self.input_cursor = 0;
    }

    pub(in crate::tui) fn move_cursor_to_end(&mut self) {
        self.input_cursor = self.command_input.len();
    }

    pub(in crate::tui) fn set_input_cursor(&mut self, cursor: usize) {
        self.break_history_navigation();
        let cursor = cursor.min(self.command_input.len());
        self.input_cursor = if self.command_input.is_char_boundary(cursor) {
            cursor
        } else {
            self.previous_char_boundary(cursor)
        };
    }

    pub(in crate::tui) fn show_completions(
        &mut self,
        title: impl Into<String>,
        items: Vec<TuiCompletionItem>,
    ) {
        self.completion = (!items.is_empty()).then(|| TuiCompletionMenu {
            title: title.into(),
            items,
            selected: 0,
        });
    }

    pub(in crate::tui) fn clear_completions(&mut self) {
        self.completion = None;
    }

    pub(in crate::tui) fn select_next_completion(&mut self) -> bool {
        let Some(menu) = self.completion.as_mut() else {
            return false;
        };
        menu.selected = (menu.selected + 1) % menu.items.len();
        true
    }

    pub(in crate::tui) fn select_previous_completion(&mut self) -> bool {
        let Some(menu) = self.completion.as_mut() else {
            return false;
        };
        menu.selected = menu.selected.checked_sub(1).unwrap_or(menu.items.len() - 1);
        true
    }

    pub(in crate::tui) fn select_completion(&mut self, index: usize) -> bool {
        let Some(menu) = self.completion.as_mut() else {
            return false;
        };
        if index >= menu.items.len() {
            return false;
        }
        menu.selected = index;
        true
    }

    pub(in crate::tui) fn accept_completion(&mut self) -> bool {
        let Some(menu) = self.completion.take() else {
            return false;
        };
        let Some(item) = menu.items.get(menu.selected) else {
            return false;
        };
        let changed = self.command_input != item.replacement;
        self.replace_command_input(item.replacement.clone());
        changed
    }

    pub(in crate::tui) fn move_cursor_to_line_start(&mut self) {
        self.input_cursor = self.command_input[..self.input_cursor]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
    }

    pub(in crate::tui) fn move_cursor_to_line_end(&mut self) {
        self.input_cursor = self.command_input[self.input_cursor..]
            .find('\n')
            .map(|offset| self.input_cursor + offset)
            .unwrap_or_else(|| self.command_input.len());
    }

    pub(in crate::tui) fn history_previous(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        if self.history_cursor.is_none() {
            self.history_draft = Some(self.command_input.clone());
        }
        let next = match self.history_cursor {
            Some(index) if index > 0 => index - 1,
            Some(index) => index,
            None => self.input_history.len() - 1,
        };
        self.history_cursor = Some(next);
        if let Some(value) = self.input_history.get(next) {
            self.command_input = value.clone();
            self.input_cursor = self.command_input.len();
        }
    }

    pub(in crate::tui) fn history_next(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };
        if index + 1 >= self.input_history.len() {
            self.history_cursor = None;
            self.command_input = self.history_draft.take().unwrap_or_default();
            self.input_cursor = self.command_input.len();
        } else {
            let next = index + 1;
            self.history_cursor = Some(next);
            if let Some(value) = self.input_history.get(next) {
                self.command_input = value.clone();
                self.input_cursor = self.command_input.len();
            }
        }
    }

    fn break_history_navigation(&mut self) {
        self.completion = None;
        self.history_cursor = None;
        self.history_draft = None;
    }

    fn previous_char_boundary(&self, index: usize) -> usize {
        self.command_input[..index]
            .char_indices()
            .last()
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn next_char_boundary(&self, index: usize) -> usize {
        self.command_input[index..]
            .chars()
            .next()
            .map(|ch| index + ch.len_utf8())
            .unwrap_or_else(|| self.command_input.len())
    }

    fn previous_word_boundary(&self, index: usize) -> usize {
        let mut cursor = index;
        while cursor > 0 {
            let previous = self.previous_char_boundary(cursor);
            let ch = self.command_input[previous..cursor]
                .chars()
                .next()
                .unwrap_or_default();
            if !ch.is_whitespace() {
                break;
            }
            cursor = previous;
        }
        while cursor > 0 {
            let previous = self.previous_char_boundary(cursor);
            let ch = self.command_input[previous..cursor]
                .chars()
                .next()
                .unwrap_or_default();
            if ch.is_whitespace() {
                break;
            }
            cursor = previous;
        }
        cursor
    }

    fn next_word_boundary(&self, index: usize) -> usize {
        let mut cursor = index;
        while cursor < self.command_input.len() {
            let next = self.next_char_boundary(cursor);
            let ch = self.command_input[cursor..next]
                .chars()
                .next()
                .unwrap_or_default();
            if ch.is_whitespace() {
                break;
            }
            cursor = next;
        }
        while cursor < self.command_input.len() {
            let next = self.next_char_boundary(cursor);
            let ch = self.command_input[cursor..next]
                .chars()
                .next()
                .unwrap_or_default();
            if !ch.is_whitespace() {
                break;
            }
            cursor = next;
        }
        cursor
    }
}
