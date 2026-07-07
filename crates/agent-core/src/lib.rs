pub mod catalog;
pub mod context;
pub mod errors;
pub mod hooks;
pub mod ids;
pub mod proposal;
pub mod run;
pub mod services;
pub mod session;
pub mod stores;
pub mod trace;

pub use catalog::{
    AgentRuntimeCatalog, PromptBlockSpec, PromptManifest, PromptManifestBlock, ToolRisk, ToolSpec,
};
pub use context::{
    CompactionRecord, ContextBlock, ContextBlockKind, ContextPolicy, ContextSnapshot,
};
pub use errors::{AgentError, AgentErrorKind, AgentErrorRecord, StoreError, ToolError};
pub use hooks::{HookEffect, HookSpec, PolicyDecision, PolicyDecisionKind};
pub use ids::{EffectId, ProposalId, RunId, SessionId, StepId, ThreadId, ToolCallId};
pub use proposal::{
    ApprovalDecision, ApprovalDecisionKind, ApprovalLevel, ProposalApprovalPolicy, ProposalDiff,
    ProposalDiffOperation, ProposalEnvelope, ProposalKindSpec, ProposalStatus, ProposalWarning,
    ProposalWarningSeverity, normalized_required_approver_count,
};
pub use run::{
    AgentRunRecord, AgentRunResult, AgentRunStatus, AgentSpec, RunCompensation, RunDependency,
    RunLease, RunRequest, RunScope, RunWorkflow, ScheduleSpec, TriggerEnvelope, TriggerKind,
    UserContext, WorkflowInputMapping, WorkflowInputTransform, WorkflowRunNode,
    WorkflowRunNodeCompensation, WorkflowRunNodeCompensationResult, WorkflowRunNodeResult,
    WorkflowRunRequest, WorkflowRunResult,
};
pub use services::{
    Agent, AgentCancellation, AgentContext, AgentEventEmitter, AgentServices, AgentStateAccess,
    ArtifactPublishRequest, ArtifactPublisher, CancellationFuture, CancellationSignal,
    ProposalCreator, SubagentRequest, SubagentRunner, ToolCaller, ToolContext, ToolRegistry,
    TraceSink,
};
pub use session::{SessionRecord, StepKind, StepRecord, ThreadRecord};
pub use stores::{
    AgentLockStore, AgentProposalStore, AgentRegistry, AgentRunEventStore, AgentRunStore,
    AgentSessionStore, AgentStateStore, AgentTraceStore, RunEventCursor, RunEventRecord,
};
pub use trace::{
    AgentEvent, AgentTrace, ArtifactKind, ArtifactRef, ArtifactStoreRef, HookEvent, HookEventName,
    HookInvocationStatus, HookKind, RedactionClassification, TraceEvent, TraceSpan,
    TraceUsageProviderSummary, TraceUsageSummary,
};

pub const PROTOCOL_VERSION: &str = "agent.v1";
pub const CATALOG_VERSION: &str = "agent_catalog.v1";

pub fn protocol_version() -> String {
    PROTOCOL_VERSION.to_owned()
}

pub fn catalog_version() -> String {
    CATALOG_VERSION.to_owned()
}
