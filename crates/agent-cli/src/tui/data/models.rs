use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) enum TranscriptRole {
    User,
    Assistant,
    System,
    Tool,
}

impl TranscriptRole {
    pub(in crate::tui) fn label(&self) -> &'static str {
        match self {
            Self::User => "You",
            Self::Assistant => "Assistant",
            Self::System => "System",
            Self::Tool => "Tool",
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct TranscriptItem {
    pub(in crate::tui) role: TranscriptRole,
    pub(in crate::tui) title: Option<String>,
    pub(in crate::tui) content: String,
    pub(in crate::tui) streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiContextStatus {
    pub(in crate::tui) snapshot_id: String,
    pub(in crate::tui) token_estimate: u32,
    pub(in crate::tui) max_input_tokens: u32,
    pub(in crate::tui) block_count: usize,
    pub(in crate::tui) omitted_block_count: u32,
    pub(in crate::tui) compacted: bool,
    pub(in crate::tui) compaction_strategy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiAgentSummary {
    pub(in crate::tui) id: String,
    pub(in crate::tui) name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiRunSummary {
    pub(in crate::tui) run_id: String,
    pub(in crate::tui) agent_id: String,
    pub(in crate::tui) status: String,
    pub(in crate::tui) started_at: String,
    pub(in crate::tui) finished_at: Option<String>,
    pub(in crate::tui) cancellation_requested: bool,
    pub(in crate::tui) error: Option<String>,
    pub(in crate::tui) input_preview: String,
    pub(in crate::tui) output_preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiWorkflowSummary {
    pub(in crate::tui) workflow_id: String,
    pub(in crate::tui) status: String,
    pub(in crate::tui) node_count: usize,
    pub(in crate::tui) completed_count: usize,
    pub(in crate::tui) failed_count: usize,
    pub(in crate::tui) skipped_count: usize,
    pub(in crate::tui) compensation_count: usize,
    pub(in crate::tui) nodes: Vec<TuiWorkflowNodeSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiWorkflowNodeSummary {
    pub(in crate::tui) node_id: String,
    pub(in crate::tui) agent_id: String,
    pub(in crate::tui) status: String,
    pub(in crate::tui) run_id: Option<String>,
    pub(in crate::tui) depends_on: Vec<String>,
    pub(in crate::tui) reason: Option<String>,
    pub(in crate::tui) blocked_dependencies: Vec<String>,
    pub(in crate::tui) compensation: Option<TuiWorkflowCompensationSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiWorkflowCompensationSummary {
    pub(in crate::tui) agent_id: String,
    pub(in crate::tui) status: String,
    pub(in crate::tui) run_id: Option<String>,
    pub(in crate::tui) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiProposalListSummary {
    pub(in crate::tui) total_count: usize,
    pub(in crate::tui) pending_count: usize,
    pub(in crate::tui) approved_count: usize,
    pub(in crate::tui) denied_count: usize,
    pub(in crate::tui) proposals: Vec<TuiProposalSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiProposalSummary {
    pub(in crate::tui) proposal_id: String,
    pub(in crate::tui) run_id: String,
    pub(in crate::tui) agent_id: String,
    pub(in crate::tui) kind: String,
    pub(in crate::tui) summary: String,
    pub(in crate::tui) status: String,
    pub(in crate::tui) risk: String,
    pub(in crate::tui) diff_count: usize,
    pub(in crate::tui) warning_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiTraceEventSummary {
    pub(in crate::tui) run_id: String,
    pub(in crate::tui) agent_id: String,
    pub(in crate::tui) event_count: usize,
    pub(in crate::tui) shown_count: usize,
    pub(in crate::tui) events: Vec<TuiTraceEventItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiTraceEventItem {
    pub(in crate::tui) kind: String,
    pub(in crate::tui) detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) enum TuiActivityKind {
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
    pub(in crate::tui) fn label(&self) -> &'static str {
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
pub(in crate::tui) struct TuiActivityItem {
    pub(in crate::tui) kind: TuiActivityKind,
    pub(in crate::tui) title: String,
    pub(in crate::tui) detail: Option<String>,
}

impl TuiActivityItem {
    pub(in crate::tui) fn new(kind: TuiActivityKind, title: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            detail: None,
        }
    }

    pub(in crate::tui) fn with_detail(
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

    pub(in crate::tui) fn line(&self) -> String {
        match self.detail.as_deref() {
            Some(detail) if !detail.is_empty() => format!("{}: {detail}", self.title),
            _ => self.title.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct TuiPendingApproval {
    pub(in crate::tui) risk: TuiToolRisk,
    pub(in crate::tui) action: TuiPendingApprovalAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum TuiApprovalSelection {
    Approve,
    Deny,
}

impl TuiApprovalSelection {
    pub(in crate::tui) fn toggled(self) -> Self {
        match self {
            Self::Approve => Self::Deny,
            Self::Deny => Self::Approve,
        }
    }

    pub(in crate::tui) fn command(self) -> &'static str {
        match self {
            Self::Approve => "yes",
            Self::Deny => "no",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum TuiFocusPanel {
    Chat,
    Context,
    Activity,
}

impl TuiFocusPanel {
    pub(in crate::tui) fn next(self) -> Self {
        match self {
            Self::Chat => Self::Context,
            Self::Context => Self::Activity,
            Self::Activity => Self::Chat,
        }
    }

    pub(in crate::tui) fn previous(self) -> Self {
        match self {
            Self::Chat => Self::Activity,
            Self::Context => Self::Chat,
            Self::Activity => Self::Context,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::tui) enum TuiDetailKind {
    #[default]
    Overview,
    Run,
    Workflow,
    Proposals,
    Events,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiCompletionItem {
    pub(in crate::tui) label: String,
    pub(in crate::tui) description: Option<String>,
    pub(in crate::tui) replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiCompletionMenu {
    pub(in crate::tui) title: String,
    pub(in crate::tui) items: Vec<TuiCompletionItem>,
    pub(in crate::tui) selected: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::tui) struct TuiPaneSizing {
    pub(in crate::tui) side_width: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) struct TuiSelectionPoint {
    pub(in crate::tui) column: u16,
    pub(in crate::tui) row: u16,
}

impl TuiSelectionPoint {
    pub(in crate::tui) fn new(column: u16, row: u16) -> Self {
        Self { column, row }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct TuiTextSelection {
    pub(in crate::tui) panel: TuiFocusPanel,
    pub(in crate::tui) anchor: TuiSelectionPoint,
    pub(in crate::tui) focus: TuiSelectionPoint,
}

impl TuiTextSelection {
    pub(in crate::tui) fn new(panel: TuiFocusPanel, point: TuiSelectionPoint) -> Self {
        Self {
            panel,
            anchor: point,
            focus: point,
        }
    }

    pub(in crate::tui) fn update(&mut self, point: TuiSelectionPoint) {
        self.focus = point;
    }

    pub(in crate::tui) fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }

    pub(in crate::tui) fn ordered_points(&self) -> (TuiSelectionPoint, TuiSelectionPoint) {
        let start_key = (self.anchor.row, self.anchor.column);
        let end_key = (self.focus.row, self.focus.column);
        if start_key <= end_key {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::tui) enum TuiPendingApprovalAction {
    SlashTool {
        tool_name: String,
        input: Value,
    },
    ChatTools {
        agent_id: String,
        state: Box<ChatTurnState>,
        tool_calls: Vec<ChatToolCall>,
        surface_messages: Vec<LlmMessage>,
    },
}

impl TuiPendingApproval {
    pub(in crate::tui) fn tool_call(
        tool_name: impl Into<String>,
        risk: TuiToolRisk,
        input: Value,
    ) -> Self {
        Self {
            risk,
            action: TuiPendingApprovalAction::SlashTool {
                tool_name: tool_name.into(),
                input,
            },
        }
    }

    pub(in crate::tui) fn chat_tools(
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
                state: Box::new(state),
                tool_calls,
                surface_messages,
            },
        }
    }

    pub(in crate::tui) fn subject(&self) -> String {
        match &self.action {
            TuiPendingApprovalAction::SlashTool { tool_name, .. } => tool_name.clone(),
            TuiPendingApprovalAction::ChatTools { tool_calls, .. } => match tool_calls.as_slice() {
                [] => "chat tools".to_owned(),
                [call] => call.name.clone(),
                [first, rest @ ..] => format!("{} +{} tool(s)", first.name, rest.len()),
            },
        }
    }

    pub(in crate::tui) fn summary(&self) -> String {
        format!("{} ({})", self.subject(), self.risk.label())
    }
}

#[derive(Debug)]
pub(in crate::tui) enum TuiUpdate {
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
    pub(crate) runtime_sources: ResolvedRuntimeSources,
    pub(crate) trace_path: Option<Utf8PathBuf>,
    pub(crate) store_path: Utf8PathBuf,
    pub(crate) store_backend: RuntimeStoreBackend,
    pub(crate) tool_overrides: ToolOverrides,
    pub(crate) allow_high_risk_tools: bool,
    pub(crate) chat: ChatLlmOptions,
    pub(crate) timeout_seconds: u64,
    pub(crate) max_retries: u32,
    pub(crate) retry_backoff_ms: u64,
    pub(crate) hooks: Vec<HookSpec>,
    pub(crate) context_policy: ContextPolicy,
    pub(crate) default_agent: Option<String>,
    pub(crate) mouse_capture: bool,
    pub(crate) once: bool,
}

pub(in crate::tui) struct TuiState {
    pub(in crate::tui) options: TuiOptions,
    pub(in crate::tui) selected_agent_id: Option<String>,
    pub(in crate::tui) agents: Vec<TuiAgentSummary>,
    pub(in crate::tui) catalog_summary: Option<CatalogSummary>,
    pub(in crate::tui) trace: Option<agent_core::AgentTrace>,
    pub(in crate::tui) trace_label: Option<String>,
    pub(in crate::tui) recent_runs: Vec<AgentRunRecord>,
    pub(in crate::tui) status: String,
    pub(in crate::tui) input_mode: bool,
    pub(in crate::tui) command_input: String,
    pub(in crate::tui) input_cursor: usize,
    pub(in crate::tui) completion: Option<TuiCompletionMenu>,
    pub(in crate::tui) transcript: Vec<TranscriptItem>,
    pub(in crate::tui) active_assistant_index: Option<usize>,
    pub(in crate::tui) events: VecDeque<String>,
    pub(in crate::tui) activity: VecDeque<TuiActivityItem>,
    pub(in crate::tui) tool_inventory: Option<TuiToolInventory>,
    pub(in crate::tui) context_status: Option<TuiContextStatus>,
    pub(in crate::tui) latest_run: Option<TuiRunSummary>,
    pub(in crate::tui) latest_workflow: Option<TuiWorkflowSummary>,
    pub(in crate::tui) latest_proposals: Option<TuiProposalListSummary>,
    pub(in crate::tui) latest_events: Option<TuiTraceEventSummary>,
    pub(in crate::tui) pending_approval: Option<TuiPendingApproval>,
    pub(in crate::tui) approval_selection: Option<TuiApprovalSelection>,
    pub(in crate::tui) chat_messages: Vec<LlmMessage>,
    pub(in crate::tui) chat_scroll: u16,
    pub(in crate::tui) context_scroll: u16,
    pub(in crate::tui) event_scroll: u16,
    pub(in crate::tui) focused_panel: TuiFocusPanel,
    pub(in crate::tui) sidebar_panel: TuiFocusPanel,
    pub(in crate::tui) detail_kind: TuiDetailKind,
    pub(in crate::tui) pane_sizing: TuiPaneSizing,
    pub(in crate::tui) text_selection: Option<TuiTextSelection>,
    pub(in crate::tui) input_history: VecDeque<String>,
    pub(in crate::tui) history_cursor: Option<usize>,
    pub(in crate::tui) history_draft: Option<String>,
    pub(in crate::tui) busy: bool,
    pub(in crate::tui) operation_label: Option<String>,
    pub(in crate::tui) operation_started_at: Option<Instant>,
}
