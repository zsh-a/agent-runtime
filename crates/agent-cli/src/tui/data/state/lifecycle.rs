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
}
