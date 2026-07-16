use super::*;

impl TuiState {
    pub(in crate::tui) async fn load(options: TuiOptions) -> Result<Self> {
        let catalog_summary = load_catalog_summary(options.runtime_sources.catalog_path()).await?;
        let trace = load_trace(options.trace_path.as_ref()).await?;
        let trace_label = options.trace_path.as_ref().map(ToString::to_string);
        let recent_runs = read_recent_runs(options.store_backend, &options.store_path).await?;
        let tool_inventory = Some(load_tui_tool_inventory(&options).await?);
        let agents = load_agent_summaries(&options).await?;
        let selected_agent_id = select_initial_agent(options.default_agent.as_deref(), &agents)?;
        let status = status_line(
            selected_agent_id.as_deref(),
            &catalog_summary,
            &trace,
            recent_runs.len(),
        );
        let mut state = Self {
            options,
            selected_agent_id,
            agents,
            catalog_summary,
            trace,
            trace_label,
            recent_runs,
            status,
            input_mode: true,
            command_input: String::new(),
            input_cursor: 0,
            completion: None,
            transcript: Vec::new(),
            active_assistant_index: None,
            events: VecDeque::new(),
            activity: VecDeque::new(),
            tool_inventory,
            context_status: None,
            latest_run: None,
            latest_workflow: None,
            latest_proposals: None,
            latest_events: None,
            pending_approval: None,
            approval_selection: None,
            chat_messages: Vec::new(),
            chat_scroll: 0,
            context_scroll: 0,
            event_scroll: 0,
            focused_panel: TuiFocusPanel::Chat,
            sidebar_panel: TuiFocusPanel::Context,
            detail_kind: TuiDetailKind::Overview,
            pane_sizing: TuiPaneSizing::default(),
            text_selection: None,
            input_history: VecDeque::new(),
            history_cursor: None,
            history_draft: None,
            busy: false,
            operation_label: None,
            operation_started_at: None,
        };
        state.push_event("ready");
        Ok(state)
    }

    pub(in crate::tui) async fn refresh(&mut self) -> Result<()> {
        self.catalog_summary =
            load_catalog_summary(self.options.runtime_sources.catalog_path()).await?;
        self.agents = load_agent_summaries(&self.options).await?;
        self.tool_inventory = Some(load_tui_tool_inventory(&self.options).await?);
        if let Some(path) = &self.options.trace_path {
            self.trace = Some(read_trace(path.clone()).await?);
            self.trace_label = Some(path.to_string());
        }
        self.refresh_runs().await?;
        Ok(())
    }

    pub(in crate::tui) async fn refresh_runs(&mut self) -> Result<()> {
        self.recent_runs =
            read_recent_runs(self.options.store_backend, &self.options.store_path).await?;
        self.update_status();
        Ok(())
    }

    pub(in crate::tui) fn set_recent_runs(&mut self, runs: Vec<AgentRunRecord>) {
        self.recent_runs = runs.into_iter().take(8).collect();
        self.update_status();
    }

    pub(in crate::tui) fn set_trace(
        &mut self,
        label: impl Into<String>,
        trace: agent_core::AgentTrace,
    ) {
        self.trace = Some(trace);
        self.trace_label = Some(label.into());
        self.update_status();
    }

    pub(in crate::tui) fn set_selected_agent(&mut self, agent_id: impl Into<String>) {
        self.selected_agent_id = Some(agent_id.into());
        self.update_status();
    }

    pub(in crate::tui) fn set_latest_workflow(&mut self, summary: TuiWorkflowSummary) {
        self.latest_workflow = Some(summary);
        self.detail_kind = TuiDetailKind::Workflow;
        self.focus_panel(TuiFocusPanel::Context);
    }

    pub(in crate::tui) fn set_latest_run(&mut self, summary: TuiRunSummary) {
        self.latest_run = Some(summary);
        self.detail_kind = TuiDetailKind::Run;
        self.focus_panel(TuiFocusPanel::Context);
    }

    pub(in crate::tui) fn set_latest_proposals(&mut self, summary: TuiProposalListSummary) {
        self.latest_proposals = Some(summary);
        self.detail_kind = TuiDetailKind::Proposals;
        self.focus_panel(TuiFocusPanel::Context);
    }

    pub(in crate::tui) fn set_latest_events(&mut self, summary: TuiTraceEventSummary) {
        self.latest_events = Some(summary);
        self.detail_kind = TuiDetailKind::Events;
        self.focus_panel(TuiFocusPanel::Context);
    }

    pub(in crate::tui) fn active_agent_label(&self) -> &str {
        self.selected_agent_id.as_deref().unwrap_or("auto")
    }

    fn update_status(&mut self) {
        self.status = status_line(
            self.selected_agent_id.as_deref(),
            &self.catalog_summary,
            &self.trace,
            self.recent_runs.len(),
        );
    }

    pub(in crate::tui) fn enter_command(&mut self, prefix: &str) {
        self.input_mode = true;
        self.completion = None;
        self.command_input.clear();
        self.command_input.push_str(prefix);
        self.input_cursor = self.command_input.len();
        self.history_cursor = None;
        self.history_draft = None;
    }

    pub(in crate::tui) fn push_event(&mut self, line: impl Into<String>) {
        let line = line.into();
        for part in line.lines() {
            self.events.push_back(part.to_owned());
            self.activity
                .push_back(TuiActivityItem::new(TuiActivityKind::System, part));
        }
        self.truncate_activity();
    }

    pub(in crate::tui) fn push_activity(&mut self, activity: TuiActivityItem) {
        self.events.push_back(activity.line());
        self.activity.push_back(activity);
        self.truncate_activity();
    }

    pub(in crate::tui) fn clear_output(&mut self) {
        self.transcript.clear();
        self.active_assistant_index = None;
        self.events.clear();
        self.activity.clear();
        self.latest_run = None;
        self.latest_workflow = None;
        self.latest_proposals = None;
        self.latest_events = None;
        self.pending_approval = None;
        self.approval_selection = None;
        self.completion = None;
        self.chat_scroll = 0;
        self.context_scroll = 0;
        self.event_scroll = 0;
    }

    pub(in crate::tui) fn push_user_message(&mut self, content: impl Into<String>) {
        self.finish_assistant_stream();
        self.active_assistant_index = None;
        self.push_transcript(TranscriptRole::User, None, content.into(), false);
    }

    pub(in crate::tui) fn push_assistant_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptRole::Assistant, None, content.into(), false);
    }

    pub(in crate::tui) fn push_system_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptRole::System, None, content.into(), false);
    }

    pub(in crate::tui) fn push_tool_message(
        &mut self,
        title: impl Into<Option<String>>,
        content: impl Into<String>,
    ) {
        self.finish_assistant_stream();
        self.active_assistant_index = None;
        self.push_transcript(TranscriptRole::Tool, title.into(), content.into(), false);
    }

    pub(in crate::tui) fn start_assistant_stream(&mut self) {
        self.finish_assistant_stream();
        self.push_transcript(TranscriptRole::Assistant, None, String::new(), true);
        self.active_assistant_index = self.transcript.len().checked_sub(1);
    }

    pub(in crate::tui) fn append_assistant_delta(&mut self, content: &str) {
        let active_streaming = self
            .active_assistant_index
            .and_then(|index| self.transcript.get(index))
            .is_some_and(|item| item.role == TranscriptRole::Assistant && item.streaming);
        if !active_streaming {
            self.start_assistant_stream();
        }
        if let Some(item) = self
            .active_assistant_index
            .and_then(|index| self.transcript.get_mut(index))
        {
            item.content.push_str(content);
        }
    }

    pub(in crate::tui) fn replace_active_assistant(&mut self, content: impl Into<String>) {
        let content = content.into();
        if let Some(item) = self
            .active_assistant_index
            .and_then(|index| self.transcript.get_mut(index))
            .filter(|item| item.role == TranscriptRole::Assistant)
        {
            item.content = content;
            item.streaming = false;
        } else if !content.is_empty() {
            self.push_assistant_message(content);
            self.active_assistant_index = self.transcript.len().checked_sub(1);
        }
    }

    pub(in crate::tui) fn finish_assistant_stream(&mut self) {
        if let Some(item) = self
            .active_assistant_index
            .and_then(|index| self.transcript.get_mut(index))
            .filter(|item| item.role == TranscriptRole::Assistant)
        {
            item.streaming = false;
        }
    }

    pub(in crate::tui) fn set_busy(&mut self, busy: bool) {
        if busy {
            if !self.busy {
                self.operation_started_at = Some(Instant::now());
            }
            self.operation_label
                .get_or_insert_with(|| "running".to_owned());
        } else {
            self.operation_label = None;
            self.operation_started_at = None;
        }
        self.busy = busy;
    }

    pub(in crate::tui) fn start_operation(&mut self, label: impl Into<String>) {
        self.operation_label = Some(label.into());
        self.operation_started_at = Some(Instant::now());
        self.busy = true;
    }

    pub(in crate::tui) fn set_operation_label(&mut self, label: impl Into<String>) {
        if self.busy {
            self.operation_label = Some(label.into());
        }
    }

    pub(in crate::tui) fn operation_status(&self) -> String {
        let label = self.operation_label.as_deref().unwrap_or("running");
        let elapsed = self
            .operation_started_at
            .map(|started| started.elapsed().as_secs())
            .unwrap_or(0);
        format!("{label} {elapsed}s")
    }

    pub(in crate::tui) fn set_pending_approval(&mut self, approval: TuiPendingApproval) {
        self.pending_approval = Some(approval);
        self.approval_selection = None;
    }

    pub(in crate::tui) fn take_pending_approval(&mut self) -> Option<TuiPendingApproval> {
        let approval = self.pending_approval.take();
        if approval.is_some() {
            self.approval_selection = None;
        }
        approval
    }

    pub(in crate::tui) fn toggle_approval_selection(&mut self) {
        if self.pending_approval.is_some() {
            self.approval_selection = Some(
                self.approval_selection
                    .map(TuiApprovalSelection::toggled)
                    .unwrap_or(TuiApprovalSelection::Deny),
            );
        }
    }

    pub(in crate::tui) fn select_approval(&mut self) {
        if self.pending_approval.is_some() {
            self.approval_selection = Some(TuiApprovalSelection::Approve);
        }
    }

    pub(in crate::tui) fn select_denial(&mut self) {
        if self.pending_approval.is_some() {
            self.approval_selection = Some(TuiApprovalSelection::Deny);
        }
    }

    pub(in crate::tui) fn approval_picker_active(&self) -> bool {
        self.pending_approval.is_some()
    }

    pub(in crate::tui) fn apply_update(&mut self, update: TuiUpdate) {
        match update {
            TuiUpdate::Activity(activity) => self.push_activity(activity),
            TuiUpdate::ContextStatus(status) => {
                self.context_status = Some(status);
            }
            TuiUpdate::PendingApproval(approval) => {
                self.approval_selection = None;
                self.pending_approval = approval;
            }
            TuiUpdate::SystemMessage(content) => self.push_system_message(content),
            TuiUpdate::AssistantDelta(content) => self.append_assistant_delta(&content),
            TuiUpdate::AssistantReplace(content) => self.replace_active_assistant(content),
            TuiUpdate::AssistantFinish => self.finish_assistant_stream(),
            TuiUpdate::ToolMessage { title, content } => self.push_tool_message(title, content),
            TuiUpdate::ChatMessages(messages) => {
                self.chat_messages = messages;
            }
            TuiUpdate::Busy(busy) => self.set_busy(busy),
            TuiUpdate::Error(message) => {
                self.replace_active_assistant(format!("Error: {message}"));
                self.push_activity(TuiActivityItem::with_detail(
                    TuiActivityKind::Error,
                    "command failed",
                    message,
                ));
            }
        }
    }

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

    fn push_transcript(
        &mut self,
        role: TranscriptRole,
        title: Option<String>,
        content: String,
        streaming: bool,
    ) {
        self.transcript.push(TranscriptItem {
            role,
            title,
            content,
            streaming,
        });
        self.chat_scroll = 0;
    }

    fn truncate_activity(&mut self) {
        while self.events.len() > MAX_EVENT_LINES {
            self.events.pop_front();
        }
        while self.activity.len() > MAX_EVENT_LINES {
            self.activity.pop_front();
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
