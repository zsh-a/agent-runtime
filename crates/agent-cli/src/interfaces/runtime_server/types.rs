use super::*;

pub(crate) struct RuntimeServerOptions {
    pub(crate) sources: ResolvedRuntimeSources,
    pub(crate) store_path: Utf8PathBuf,
    pub(crate) store_backend: RuntimeStoreBackend,
    pub(crate) tool_overrides: ToolOverrides,
    pub(crate) hooks: HookManager,
    pub(crate) context_policy: ContextPolicy,
    pub(crate) default_agent: Option<String>,
    pub(crate) chat: ChatLlmOptions,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentRunResponse {
    pub(crate) result: AgentRunResult,
    pub(crate) trace: agent_core::AgentTrace,
}

#[derive(Debug, Serialize)]
pub(crate) struct ToolCallResponse {
    pub(super) tool: String,
    pub(super) output: Value,
}

#[derive(Clone)]
pub(crate) struct ActiveRun {
    pub(super) cancellation: CancellationToken,
    pub(super) events: broadcast::Sender<TraceEvent>,
    pub(super) event_buffer: Arc<TraceEventBuffer>,
}

pub(crate) struct ActiveRunEvents {
    pub(crate) receiver: broadcast::Receiver<TraceEvent>,
    pub(crate) replayed_events: Vec<TraceEvent>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CancelRunResponse {
    pub(crate) cancellation_requested: bool,
    pub(crate) message: String,
    pub(crate) run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<AgentRunStatus>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpProposalCreateParams {
    pub(crate) run_id: String,
    pub(crate) agent_id: String,
    pub(crate) kind: String,
    pub(crate) summary: String,
    #[serde(default)]
    pub(crate) payload: Value,
    #[serde(default)]
    pub(crate) diffs: Vec<ProposalDiff>,
    #[serde(default)]
    pub(crate) warnings: Vec<ProposalWarning>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpProposalListParams {
    #[serde(default)]
    pub(crate) run_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpRunListParams {
    #[serde(default)]
    pub(crate) agent_id: Option<String>,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpProposalDecisionParams {
    pub(crate) decision: String,
    #[serde(default)]
    pub(crate) approval_level: Option<ApprovalLevel>,
    #[serde(default)]
    pub(crate) decided_by: Option<String>,
    #[serde(default)]
    pub(crate) comment: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AgentRunParams {
    pub(crate) agent_id: String,
    #[serde(flatten)]
    pub(crate) run: HttpAgentRunParams,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpAgentRunParams {
    #[serde(default)]
    pub(crate) run_id: Option<String>,
    #[serde(default)]
    pub(crate) input: Value,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) thread_id: Option<String>,
    #[serde(default = "default_agent_run_trigger")]
    pub(crate) trigger: TriggerKind,
    #[serde(default)]
    pub(crate) trigger_envelope: Option<TriggerEnvelope>,
    #[serde(default)]
    pub(crate) workflow: Option<RunWorkflow>,
    #[serde(default)]
    pub(crate) user: Option<UserContext>,
    #[serde(default)]
    pub(crate) scope: Option<RunScope>,
    #[serde(default)]
    pub(crate) metadata: Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpToolCallParams {
    #[serde(default)]
    pub(crate) input: Value,
}
