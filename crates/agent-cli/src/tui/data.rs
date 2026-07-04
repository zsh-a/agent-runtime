use std::collections::VecDeque;

use agent_chat::{ChatToolCall, ChatTurnState};
use agent_core::{AgentRunRecord, AgentSpec, ContextPolicy, HookSpec};
use agent_llm::LlmMessage;
use camino::{Utf8Path, Utf8PathBuf};
use miette::{IntoDiagnostic, Result, miette};
use serde_json::Value;

use crate::{
    catalog::{CatalogSummary, read_catalog},
    chat::ChatLlmOptions,
    registry::load_registry,
    tools::ToolOverrides,
};

use super::{
    policy::TuiToolRisk,
    tool_inventory::{TuiToolInventory, load_tui_tool_inventory},
};

const MAX_EVENT_LINES: usize = 160;
const MAX_HISTORY_ITEMS: usize = 80;
const SCROLL_LINES: u16 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TranscriptRole {
    User,
    Assistant,
    System,
    Tool,
}

impl TranscriptRole {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::User => "You",
            Self::Assistant => "Assistant",
            Self::System => "System",
            Self::Tool => "Tool",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct TranscriptItem {
    pub(super) role: TranscriptRole,
    pub(super) title: Option<String>,
    pub(super) content: String,
    pub(super) streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiContextStatus {
    pub(super) snapshot_id: String,
    pub(super) token_estimate: u32,
    pub(super) max_input_tokens: u32,
    pub(super) block_count: usize,
    pub(super) omitted_block_count: u32,
    pub(super) compacted: bool,
    pub(super) compaction_strategy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiAgentSummary {
    pub(super) id: String,
    pub(super) name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiRunSummary {
    pub(super) run_id: String,
    pub(super) agent_id: String,
    pub(super) status: String,
    pub(super) started_at: String,
    pub(super) finished_at: Option<String>,
    pub(super) cancellation_requested: bool,
    pub(super) error: Option<String>,
    pub(super) input_preview: String,
    pub(super) output_preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiWorkflowSummary {
    pub(super) workflow_id: String,
    pub(super) status: String,
    pub(super) node_count: usize,
    pub(super) completed_count: usize,
    pub(super) failed_count: usize,
    pub(super) skipped_count: usize,
    pub(super) compensation_count: usize,
    pub(super) nodes: Vec<TuiWorkflowNodeSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiWorkflowNodeSummary {
    pub(super) node_id: String,
    pub(super) agent_id: String,
    pub(super) status: String,
    pub(super) run_id: Option<String>,
    pub(super) depends_on: Vec<String>,
    pub(super) reason: Option<String>,
    pub(super) blocked_dependencies: Vec<String>,
    pub(super) compensation: Option<TuiWorkflowCompensationSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiWorkflowCompensationSummary {
    pub(super) agent_id: String,
    pub(super) status: String,
    pub(super) run_id: Option<String>,
    pub(super) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiProposalListSummary {
    pub(super) total_count: usize,
    pub(super) pending_count: usize,
    pub(super) approved_count: usize,
    pub(super) denied_count: usize,
    pub(super) proposals: Vec<TuiProposalSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiProposalSummary {
    pub(super) proposal_id: String,
    pub(super) run_id: String,
    pub(super) agent_id: String,
    pub(super) kind: String,
    pub(super) summary: String,
    pub(super) status: String,
    pub(super) risk: String,
    pub(super) diff_count: usize,
    pub(super) warning_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiTraceEventSummary {
    pub(super) run_id: String,
    pub(super) agent_id: String,
    pub(super) event_count: usize,
    pub(super) shown_count: usize,
    pub(super) events: Vec<TuiTraceEventItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiTraceEventItem {
    pub(super) kind: String,
    pub(super) detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TuiActivityKind {
    System,
    Chat,
    Tool,
    Context,
    Policy,
    Approval,
    Run,
    Cancellation,
    Error,
}

impl TuiActivityKind {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Chat => "chat",
            Self::Tool => "tool",
            Self::Context => "context",
            Self::Policy => "policy",
            Self::Approval => "approve",
            Self::Run => "run",
            Self::Cancellation => "cancel",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiActivityItem {
    pub(super) kind: TuiActivityKind,
    pub(super) title: String,
    pub(super) detail: Option<String>,
}

impl TuiActivityItem {
    pub(super) fn new(kind: TuiActivityKind, title: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            detail: None,
        }
    }

    pub(super) fn with_detail(
        kind: TuiActivityKind,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            title: title.into(),
            detail: Some(detail.into()),
        }
    }

    pub(super) fn line(&self) -> String {
        match self.detail.as_deref() {
            Some(detail) if !detail.is_empty() => format!("{}: {detail}", self.title),
            _ => self.title.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct TuiPendingApproval {
    pub(super) risk: TuiToolRisk,
    pub(super) action: TuiPendingApprovalAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TuiApprovalSelection {
    Approve,
    Deny,
}

impl TuiApprovalSelection {
    pub(super) fn toggled(self) -> Self {
        match self {
            Self::Approve => Self::Deny,
            Self::Deny => Self::Approve,
        }
    }

    pub(super) fn display(self) -> &'static str {
        match self {
            Self::Approve => "Approve",
            Self::Deny => "Deny",
        }
    }

    pub(super) fn command(self) -> &'static str {
        match self {
            Self::Approve => "yes",
            Self::Deny => "no",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum TuiPendingApprovalAction {
    SlashTool {
        tool_name: String,
        input: Value,
    },
    ChatTools {
        agent_id: String,
        state: ChatTurnState,
        tool_calls: Vec<ChatToolCall>,
        surface_messages: Vec<LlmMessage>,
    },
}

impl TuiPendingApproval {
    pub(super) fn tool_call(tool_name: impl Into<String>, risk: TuiToolRisk, input: Value) -> Self {
        Self {
            risk,
            action: TuiPendingApprovalAction::SlashTool {
                tool_name: tool_name.into(),
                input,
            },
        }
    }

    pub(super) fn chat_tools(
        agent_id: impl Into<String>,
        risk: TuiToolRisk,
        state: ChatTurnState,
        tool_calls: Vec<ChatToolCall>,
        surface_messages: Vec<LlmMessage>,
    ) -> Self {
        Self {
            risk,
            action: TuiPendingApprovalAction::ChatTools {
                agent_id: agent_id.into(),
                state,
                tool_calls,
                surface_messages,
            },
        }
    }

    pub(super) fn subject(&self) -> String {
        match &self.action {
            TuiPendingApprovalAction::SlashTool { tool_name, .. } => tool_name.clone(),
            TuiPendingApprovalAction::ChatTools { tool_calls, .. } => match tool_calls.as_slice() {
                [] => "chat tools".to_owned(),
                [call] => call.name.clone(),
                [first, rest @ ..] => format!("{} +{} tool(s)", first.name, rest.len()),
            },
        }
    }

    pub(super) fn summary(&self) -> String {
        format!("{} ({})", self.subject(), self.risk.label())
    }
}

#[derive(Debug)]
pub(super) enum TuiUpdate {
    Activity(TuiActivityItem),
    ContextStatus(TuiContextStatus),
    PendingApproval(Option<TuiPendingApproval>),
    SystemMessage(String),
    AssistantDelta(String),
    AssistantReplace(String),
    AssistantFinish,
    ToolMessage {
        title: Option<String>,
        content: String,
    },
    ChatMessages(Vec<LlmMessage>),
    Busy(bool),
    Error(String),
}

#[derive(Debug, Clone)]
pub(crate) struct TuiOptions {
    pub(crate) catalog_path: Option<Utf8PathBuf>,
    pub(crate) trace_path: Option<Utf8PathBuf>,
    pub(crate) store_path: Utf8PathBuf,
    pub(crate) registry_path: Utf8PathBuf,
    pub(crate) tool_overrides: ToolOverrides,
    pub(crate) allow_high_risk_tools: bool,
    pub(crate) chat: ChatLlmOptions,
    pub(crate) timeout_seconds: u64,
    pub(crate) max_retries: u32,
    pub(crate) retry_backoff_ms: u64,
    pub(crate) hooks: Vec<HookSpec>,
    pub(crate) context_policy: ContextPolicy,
    pub(crate) mouse_capture: bool,
    pub(crate) once: bool,
}

pub(super) struct TuiState {
    pub(super) options: TuiOptions,
    pub(super) selected_agent_id: Option<String>,
    pub(super) agents: Vec<TuiAgentSummary>,
    pub(super) catalog_summary: Option<CatalogSummary>,
    pub(super) trace: Option<agent_core::AgentTrace>,
    pub(super) trace_label: Option<String>,
    pub(super) recent_runs: Vec<AgentRunRecord>,
    pub(super) status: String,
    pub(super) input_mode: bool,
    pub(super) command_input: String,
    pub(super) input_cursor: usize,
    pub(super) transcript: Vec<TranscriptItem>,
    pub(super) active_assistant_index: Option<usize>,
    pub(super) events: VecDeque<String>,
    pub(super) activity: VecDeque<TuiActivityItem>,
    pub(super) tool_inventory: Option<TuiToolInventory>,
    pub(super) context_status: Option<TuiContextStatus>,
    pub(super) latest_run: Option<TuiRunSummary>,
    pub(super) latest_workflow: Option<TuiWorkflowSummary>,
    pub(super) latest_proposals: Option<TuiProposalListSummary>,
    pub(super) latest_events: Option<TuiTraceEventSummary>,
    pub(super) pending_approval: Option<TuiPendingApproval>,
    pub(super) approval_selection: TuiApprovalSelection,
    pub(super) chat_messages: Vec<LlmMessage>,
    pub(super) chat_scroll: u16,
    pub(super) event_scroll: u16,
    pub(super) input_history: VecDeque<String>,
    pub(super) history_cursor: Option<usize>,
    pub(super) history_draft: Option<String>,
    pub(super) busy: bool,
}

impl TuiState {
    pub(super) async fn load(options: TuiOptions) -> Result<Self> {
        let catalog_summary = load_catalog_summary(options.catalog_path.as_ref()).await?;
        let trace = load_trace(options.trace_path.as_ref()).await?;
        let trace_label = options.trace_path.as_ref().map(ToString::to_string);
        let recent_runs = read_recent_runs(&options.store_path).await?;
        let tool_inventory = Some(load_tui_tool_inventory(&options).await?);
        let agents = load_agent_summaries(&options).await?;
        let selected_agent_id = agents.first().map(|agent| agent.id.clone());
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
            approval_selection: TuiApprovalSelection::Approve,
            chat_messages: Vec::new(),
            chat_scroll: 0,
            event_scroll: 0,
            input_history: VecDeque::new(),
            history_cursor: None,
            history_draft: None,
            busy: false,
        };
        state.push_system_message(startup_message(&state));
        state.push_event("ready");
        Ok(state)
    }

    pub(super) async fn refresh(&mut self) -> Result<()> {
        self.catalog_summary = load_catalog_summary(self.options.catalog_path.as_ref()).await?;
        self.agents = load_agent_summaries(&self.options).await?;
        self.tool_inventory = Some(load_tui_tool_inventory(&self.options).await?);
        if let Some(path) = &self.options.trace_path {
            self.trace = Some(read_trace(path.clone()).await?);
            self.trace_label = Some(path.to_string());
        }
        self.refresh_runs().await?;
        Ok(())
    }

    pub(super) async fn refresh_runs(&mut self) -> Result<()> {
        self.recent_runs = read_recent_runs(&self.options.store_path).await?;
        self.update_status();
        Ok(())
    }

    pub(super) fn set_recent_runs(&mut self, runs: Vec<AgentRunRecord>) {
        self.recent_runs = runs.into_iter().take(8).collect();
        self.update_status();
    }

    pub(super) fn set_trace(&mut self, label: impl Into<String>, trace: agent_core::AgentTrace) {
        self.trace = Some(trace);
        self.trace_label = Some(label.into());
        self.update_status();
    }

    pub(super) fn set_selected_agent(&mut self, agent_id: impl Into<String>) {
        self.selected_agent_id = Some(agent_id.into());
        self.update_status();
    }

    pub(super) fn set_latest_workflow(&mut self, summary: TuiWorkflowSummary) {
        self.latest_workflow = Some(summary);
    }

    pub(super) fn set_latest_run(&mut self, summary: TuiRunSummary) {
        self.latest_run = Some(summary);
    }

    pub(super) fn set_latest_proposals(&mut self, summary: TuiProposalListSummary) {
        self.latest_proposals = Some(summary);
    }

    pub(super) fn set_latest_events(&mut self, summary: TuiTraceEventSummary) {
        self.latest_events = Some(summary);
    }

    pub(super) fn active_agent_label(&self) -> &str {
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

    pub(super) fn enter_command(&mut self, prefix: &str) {
        self.input_mode = true;
        self.command_input.clear();
        self.command_input.push_str(prefix);
        self.input_cursor = self.command_input.len();
        self.history_cursor = None;
        self.history_draft = None;
    }

    pub(super) fn push_event(&mut self, line: impl Into<String>) {
        let line = line.into();
        for part in line.lines() {
            self.events.push_back(part.to_owned());
            self.activity
                .push_back(TuiActivityItem::new(TuiActivityKind::System, part));
        }
        self.truncate_activity();
    }

    pub(super) fn push_activity(&mut self, activity: TuiActivityItem) {
        self.events.push_back(activity.line());
        self.activity.push_back(activity);
        self.truncate_activity();
    }

    pub(super) fn clear_output(&mut self) {
        self.transcript.clear();
        self.active_assistant_index = None;
        self.events.clear();
        self.activity.clear();
        self.latest_run = None;
        self.latest_workflow = None;
        self.latest_proposals = None;
        self.latest_events = None;
        self.pending_approval = None;
        self.approval_selection = TuiApprovalSelection::Approve;
        self.chat_scroll = 0;
        self.event_scroll = 0;
    }

    pub(super) fn push_user_message(&mut self, content: impl Into<String>) {
        self.finish_assistant_stream();
        self.active_assistant_index = None;
        self.push_transcript(TranscriptRole::User, None, content.into(), false);
    }

    pub(super) fn push_assistant_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptRole::Assistant, None, content.into(), false);
    }

    pub(super) fn push_system_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptRole::System, None, content.into(), false);
    }

    pub(super) fn push_tool_message(
        &mut self,
        title: impl Into<Option<String>>,
        content: impl Into<String>,
    ) {
        self.finish_assistant_stream();
        self.active_assistant_index = None;
        self.push_transcript(TranscriptRole::Tool, title.into(), content.into(), false);
    }

    pub(super) fn start_assistant_stream(&mut self) {
        self.finish_assistant_stream();
        self.push_transcript(TranscriptRole::Assistant, None, String::new(), true);
        self.active_assistant_index = self.transcript.len().checked_sub(1);
    }

    pub(super) fn append_assistant_delta(&mut self, content: &str) {
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

    pub(super) fn replace_active_assistant(&mut self, content: impl Into<String>) {
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

    pub(super) fn finish_assistant_stream(&mut self) {
        if let Some(item) = self
            .active_assistant_index
            .and_then(|index| self.transcript.get_mut(index))
            .filter(|item| item.role == TranscriptRole::Assistant)
        {
            item.streaming = false;
        }
    }

    pub(super) fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    pub(super) fn set_pending_approval(&mut self, approval: TuiPendingApproval) {
        self.pending_approval = Some(approval);
        self.approval_selection = TuiApprovalSelection::Approve;
    }

    pub(super) fn take_pending_approval(&mut self) -> Option<TuiPendingApproval> {
        let approval = self.pending_approval.take();
        if approval.is_some() {
            self.approval_selection = TuiApprovalSelection::Approve;
        }
        approval
    }

    pub(super) fn toggle_approval_selection(&mut self) {
        if self.pending_approval.is_some() {
            self.approval_selection = self.approval_selection.toggled();
        }
    }

    pub(super) fn select_approval(&mut self) {
        if self.pending_approval.is_some() {
            self.approval_selection = TuiApprovalSelection::Approve;
        }
    }

    pub(super) fn select_denial(&mut self) {
        if self.pending_approval.is_some() {
            self.approval_selection = TuiApprovalSelection::Deny;
        }
    }

    pub(super) fn approval_picker_active(&self) -> bool {
        self.pending_approval.is_some() && self.command_input.trim().is_empty()
    }

    pub(super) fn apply_update(&mut self, update: TuiUpdate) {
        match update {
            TuiUpdate::Activity(activity) => self.push_activity(activity),
            TuiUpdate::ContextStatus(status) => {
                self.context_status = Some(status);
            }
            TuiUpdate::PendingApproval(approval) => {
                self.approval_selection = TuiApprovalSelection::Approve;
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

    pub(super) fn remember_input(&mut self, input: impl Into<String>) {
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

    pub(super) fn replace_command_input(&mut self, input: impl Into<String>) {
        self.command_input = input.into();
        self.input_cursor = self.command_input.len();
        self.history_cursor = None;
        self.history_draft = None;
    }

    pub(super) fn clear_command_input(&mut self) {
        self.command_input.clear();
        self.input_cursor = 0;
        self.history_cursor = None;
        self.history_draft = None;
    }

    pub(super) fn take_submitted_input(&mut self) -> String {
        let input = self.command_input.trim().to_owned();
        self.clear_command_input();
        input
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        self.break_history_navigation();
        self.command_input.insert(self.input_cursor, ch);
        self.input_cursor += ch.len_utf8();
    }

    pub(super) fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub(super) fn backspace(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        self.break_history_navigation();
        let previous = self.previous_char_boundary(self.input_cursor);
        self.command_input.drain(previous..self.input_cursor);
        self.input_cursor = previous;
    }

    pub(super) fn delete(&mut self) {
        if self.input_cursor >= self.command_input.len() {
            return;
        }
        self.break_history_navigation();
        let next = self.next_char_boundary(self.input_cursor);
        self.command_input.drain(self.input_cursor..next);
    }

    pub(super) fn delete_before_cursor(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        self.break_history_navigation();
        self.command_input.drain(..self.input_cursor);
        self.input_cursor = 0;
    }

    pub(super) fn delete_after_cursor(&mut self) {
        if self.input_cursor >= self.command_input.len() {
            return;
        }
        self.break_history_navigation();
        self.command_input.drain(self.input_cursor..);
    }

    pub(super) fn delete_previous_word(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        self.break_history_navigation();
        let start = self.previous_word_boundary(self.input_cursor);
        self.command_input.drain(start..self.input_cursor);
        self.input_cursor = start;
    }

    pub(super) fn move_cursor_left(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor = self.previous_char_boundary(self.input_cursor);
        }
    }

    pub(super) fn move_cursor_right(&mut self) {
        if self.input_cursor < self.command_input.len() {
            self.input_cursor = self.next_char_boundary(self.input_cursor);
        }
    }

    pub(super) fn move_cursor_word_left(&mut self) {
        self.input_cursor = self.previous_word_boundary(self.input_cursor);
    }

    pub(super) fn move_cursor_word_right(&mut self) {
        self.input_cursor = self.next_word_boundary(self.input_cursor);
    }

    pub(super) fn move_cursor_to_start(&mut self) {
        self.input_cursor = 0;
    }

    pub(super) fn move_cursor_to_end(&mut self) {
        self.input_cursor = self.command_input.len();
    }

    pub(super) fn move_cursor_to_line_start(&mut self) {
        self.input_cursor = self.command_input[..self.input_cursor]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
    }

    pub(super) fn move_cursor_to_line_end(&mut self) {
        self.input_cursor = self.command_input[self.input_cursor..]
            .find('\n')
            .map(|offset| self.input_cursor + offset)
            .unwrap_or_else(|| self.command_input.len());
    }

    pub(super) fn history_previous(&mut self) {
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

    pub(super) fn history_next(&mut self) {
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

    pub(super) fn scroll_chat_up(&mut self) {
        self.chat_scroll = self.chat_scroll.saturating_add(SCROLL_LINES);
    }

    pub(super) fn scroll_chat_down(&mut self) {
        self.chat_scroll = self.chat_scroll.saturating_sub(SCROLL_LINES);
    }

    pub(super) fn scroll_activity_up(&mut self) {
        self.event_scroll = self.event_scroll.saturating_add(SCROLL_LINES);
    }

    pub(super) fn scroll_activity_down(&mut self) {
        self.event_scroll = self.event_scroll.saturating_sub(SCROLL_LINES);
    }

    pub(super) fn scroll_chat_top(&mut self) {
        self.chat_scroll = u16::MAX / 2;
    }

    pub(super) fn scroll_chat_bottom(&mut self) {
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

async fn load_catalog_summary(path: Option<&Utf8PathBuf>) -> Result<Option<CatalogSummary>> {
    match path {
        Some(path) => Ok(Some(CatalogSummary::from_catalog(
            &read_catalog(path.clone()).await?,
        ))),
        None => Ok(None),
    }
}

async fn load_agent_summaries(options: &TuiOptions) -> Result<Vec<TuiAgentSummary>> {
    if let Some(path) = &options.catalog_path {
        let catalog = read_catalog(path.clone()).await?;
        return Ok(agent_summaries(catalog.agents.iter()));
    }
    let registry = load_registry(options.registry_path.clone()).await?;
    let specs = registry.list_specs();
    Ok(agent_summaries(specs.iter()))
}

fn agent_summaries<'a>(agents: impl IntoIterator<Item = &'a AgentSpec>) -> Vec<TuiAgentSummary> {
    agents
        .into_iter()
        .map(|agent| TuiAgentSummary {
            id: agent.id.clone(),
            name: agent.name.clone(),
        })
        .collect()
}

async fn load_trace(path: Option<&Utf8PathBuf>) -> Result<Option<agent_core::AgentTrace>> {
    match path {
        Some(path) => Ok(Some(read_trace(path.clone()).await?)),
        None => Ok(None),
    }
}

pub(super) async fn read_recent_runs(store_path: &Utf8Path) -> Result<Vec<AgentRunRecord>> {
    let runs_dir = store_path.join("runs");
    if !runs_dir.exists() {
        return Ok(vec![]);
    }
    let mut entries = fs_err::tokio::read_dir(&runs_dir)
        .await
        .map_err(|e| miette!("failed to read runs at {runs_dir}: {e}"))?;
    let mut records = Vec::new();
    while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|path| miette!("non-UTF-8 run path: {}", path.display()))?;
        if path.extension() != Some("json") {
            continue;
        }
        let record = serde_json::from_value::<AgentRunRecord>(read_json(path).await?)
            .map_err(|e| miette!("failed to parse run record: {e}"))?;
        records.push(record);
    }
    records.sort_by_key(|record| record.started_at);
    records.reverse();
    records.truncate(8);
    Ok(records)
}

pub(super) async fn read_trace(path: Utf8PathBuf) -> Result<agent_core::AgentTrace> {
    let value = read_json(path.clone()).await?;
    serde_json::from_value(value).map_err(|e| miette!("failed to parse trace at {path}: {e}"))
}

async fn read_json(path: Utf8PathBuf) -> Result<Value> {
    let bytes = fs_err::tokio::read(&path)
        .await
        .map_err(|e| miette!("failed to read JSON at {path}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| miette!("failed to parse JSON at {path}: {e}"))
}

fn status_line(
    selected_agent_id: Option<&str>,
    catalog_summary: &Option<CatalogSummary>,
    trace: &Option<agent_core::AgentTrace>,
    run_count: usize,
) -> String {
    format!(
        "agent {} | catalog {} | trace {} | runs {}",
        selected_agent_id.unwrap_or("auto"),
        catalog_summary
            .as_ref()
            .map(|summary| summary.agent_count.to_string())
            .unwrap_or_else(|| "-".to_owned()),
        trace
            .as_ref()
            .map(|trace| trace.run_id.0.clone())
            .unwrap_or_else(|| "-".to_owned()),
        run_count
    )
}

fn startup_message(state: &TuiState) -> String {
    let tool_count = state
        .tool_inventory
        .as_ref()
        .map(TuiToolInventory::total_count)
        .unwrap_or(0);
    format!(
        "Ready. Chatting with agent '{}'.\n\n\
        Type a message and press Enter, or use slash commands.\n\
        Quick commands: /status, /runs, /tools, /help <command>.\n\
        Tab completes commands, agents, tools, runs, proposals, and help topics.\n\
        Model: {} / {}. Tools: {}. Recent runs: {}.",
        state.active_agent_label(),
        state.options.chat.provider,
        state.options.chat.model,
        tool_count,
        state.recent_runs.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_chat::{ChatToolExecution, ChatTurnRequest, chat_turn_initial_state};
    use agent_core::PROTOCOL_VERSION;
    use agent_llm::user_message;
    use serde_json::json;

    fn test_state() -> TuiState {
        TuiState {
            options: TuiOptions {
                catalog_path: None,
                trace_path: None,
                store_path: Utf8PathBuf::from("store"),
                registry_path: Utf8PathBuf::from("agents.yaml"),
                tool_overrides: ToolOverrides::default(),
                allow_high_risk_tools: true,
                chat: ChatLlmOptions {
                    provider: "mock".to_owned(),
                    model: "mock-model".to_owned(),
                    mock_response: "ok".to_owned(),
                    api_base_url: None,
                    api_key_env: "OPENAI_API_KEY".to_owned(),
                    anthropic_version: "2023-06-01".to_owned(),
                    temperature: None,
                    max_output_tokens: None,
                    max_tool_rounds: 4,
                },
                timeout_seconds: 60,
                max_retries: 0,
                retry_backoff_ms: 0,
                hooks: Vec::new(),
                context_policy: ContextPolicy::default(),
                mouse_capture: false,
                once: false,
            },
            selected_agent_id: Some("echo_agent".to_owned()),
            agents: vec![TuiAgentSummary {
                id: "echo_agent".to_owned(),
                name: "Echo Agent".to_owned(),
            }],
            catalog_summary: None,
            trace: None,
            trace_label: None,
            recent_runs: Vec::new(),
            status: "ready".to_owned(),
            input_mode: true,
            command_input: String::new(),
            input_cursor: 0,
            transcript: Vec::new(),
            active_assistant_index: None,
            events: VecDeque::new(),
            activity: VecDeque::new(),
            tool_inventory: None,
            context_status: None,
            latest_run: None,
            latest_workflow: None,
            latest_proposals: None,
            latest_events: None,
            pending_approval: None,
            approval_selection: TuiApprovalSelection::Approve,
            chat_messages: Vec::new(),
            chat_scroll: 0,
            event_scroll: 0,
            input_history: VecDeque::new(),
            history_cursor: None,
            history_draft: None,
            busy: false,
        }
    }

    fn test_chat_state() -> ChatTurnState {
        chat_turn_initial_state(&ChatTurnRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            turn_id: None,
            surface: Some("agent_tui".to_owned()),
            mode: Some("natural_language".to_owned()),
            session_id: None,
            thread_id: None,
            agent_id: Some("echo_agent".to_owned()),
            provider: "mock".to_owned(),
            model: "mock-model".to_owned(),
            messages: vec![user_message("hello")],
            temperature: None,
            max_output_tokens: None,
            tools: Vec::new(),
            metadata: json!({}),
            context_policy: Default::default(),
            max_tool_rounds: 4,
            tool_execution: ChatToolExecution::Client,
        })
        .expect("chat state")
    }

    #[test]
    fn startup_message_summarizes_next_steps() {
        let state = test_state();

        let message = startup_message(&state);

        assert!(message.contains("Ready. Chatting with agent 'echo_agent'."));
        assert!(message.contains("Quick commands: /status, /runs, /tools, /help <command>."));
        assert!(
            message.contains(
                "Tab completes commands, agents, tools, runs, proposals, and help topics."
            )
        );
        assert!(message.contains("Model: mock / mock-model."));
    }

    #[test]
    fn command_input_edits_at_cursor() {
        let mut state = test_state();
        state.replace_command_input("hello");

        state.move_cursor_left();
        state.move_cursor_left();
        state.insert_char('X');
        assert_eq!(state.command_input, "helXlo");

        state.backspace();
        assert_eq!(state.command_input, "hello");

        state.move_cursor_to_start();
        state.delete();
        assert_eq!(state.command_input, "ello");
        assert_eq!(state.input_cursor, 0);
    }

    #[test]
    fn command_history_restores_unsubmitted_draft() {
        let mut state = test_state();
        state.remember_input("first");
        state.remember_input("second");
        state.replace_command_input("draft");

        state.history_previous();
        assert_eq!(state.command_input, "second");
        state.history_previous();
        assert_eq!(state.command_input, "first");
        state.history_next();
        assert_eq!(state.command_input, "second");
        state.history_next();
        assert_eq!(state.command_input, "draft");
        assert_eq!(state.history_cursor, None);
    }

    #[test]
    fn assistant_replace_updates_stream_after_done() {
        let mut state = test_state();
        state.start_assistant_stream();
        state.apply_update(TuiUpdate::AssistantDelta("partial".to_owned()));
        state.apply_update(TuiUpdate::AssistantFinish);
        state.apply_update(TuiUpdate::AssistantReplace("final answer".to_owned()));

        let assistant_items = state
            .transcript
            .iter()
            .filter(|item| item.role == TranscriptRole::Assistant)
            .collect::<Vec<_>>();
        assert_eq!(assistant_items.len(), 1);
        assert_eq!(assistant_items[0].content, "final answer");
        assert!(!assistant_items[0].streaming);
    }

    #[test]
    fn assistant_replace_after_tool_result_keeps_final_answer_visible() {
        let mut state = test_state();
        state.start_assistant_stream();
        state.apply_update(TuiUpdate::AssistantDelta("checking".to_owned()));
        state.apply_update(TuiUpdate::ToolMessage {
            title: Some("echo".to_owned()),
            content: "tool result".to_owned(),
        });
        state.apply_update(TuiUpdate::AssistantReplace("final answer".to_owned()));

        let assistant_items = state
            .transcript
            .iter()
            .filter(|item| item.role == TranscriptRole::Assistant)
            .collect::<Vec<_>>();
        assert_eq!(assistant_items.len(), 2);
        assert_eq!(assistant_items[0].content, "checking");
        assert_eq!(assistant_items[1].content, "final answer");
    }

    #[test]
    fn pending_approval_summary_names_slash_and_chat_tools() {
        let slash = TuiPendingApproval::tool_call("agent.run", TuiToolRisk::High, json!({}));
        assert_eq!(slash.subject(), "agent.run");
        assert_eq!(slash.summary(), "agent.run (high)");

        let chat = TuiPendingApproval::chat_tools(
            "echo_agent",
            TuiToolRisk::High,
            test_chat_state(),
            vec![
                ChatToolCall {
                    id: "call_1".to_owned(),
                    name: "agent.run".to_owned(),
                    input: json!({}),
                },
                ChatToolCall {
                    id: "call_2".to_owned(),
                    name: "echo".to_owned(),
                    input: json!({}),
                },
            ],
            vec![user_message("hello")],
        );
        assert_eq!(chat.subject(), "agent.run +1 tool(s)");
        assert_eq!(chat.summary(), "agent.run +1 tool(s) (high)");
    }

    #[test]
    fn pending_approval_update_sets_and_clears_state() {
        let mut state = test_state();
        let approval = TuiPendingApproval::tool_call("agent.run", TuiToolRisk::High, json!({}));

        state.apply_update(TuiUpdate::PendingApproval(Some(approval)));
        assert_eq!(
            state
                .pending_approval
                .as_ref()
                .map(TuiPendingApproval::summary),
            Some("agent.run (high)".to_owned())
        );

        state.apply_update(TuiUpdate::PendingApproval(None));
        assert!(state.pending_approval.is_none());
    }
}
