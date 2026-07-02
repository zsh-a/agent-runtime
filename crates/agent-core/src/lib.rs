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
pub use ids::{ProposalId, RunId, SessionId, StepId, ThreadId, ToolCallId};
pub use proposal::{
    ApprovalDecision, ApprovalDecisionKind, ProposalApprovalPolicy, ProposalEnvelope,
    ProposalKindSpec, ProposalStatus,
};
pub use run::{
    AgentRunRecord, AgentRunResult, AgentRunStatus, AgentSpec, RunLease, RunRequest, RunScope,
    ScheduleSpec, TriggerKind, UserContext,
};
pub use services::{Agent, AgentContext, AgentServices, ToolContext, ToolRegistry, TraceSink};
pub use session::{SessionRecord, StepKind, StepRecord, ThreadRecord};
pub use stores::{
    AgentLockStore, AgentProposalStore, AgentRegistry, AgentRunStore, AgentSessionStore,
    AgentStateStore,
};
pub use trace::{
    AgentEvent, AgentTrace, ArtifactRef, HookEvent, HookEventName, HookInvocationStatus, HookKind,
    TraceEvent,
};

pub const PROTOCOL_VERSION: &str = "agent.v1";
pub const CATALOG_VERSION: &str = "agent_catalog.v1";

pub fn protocol_version() -> String {
    PROTOCOL_VERSION.to_owned()
}

pub fn catalog_version() -> String {
    CATALOG_VERSION.to_owned()
}
