use std::{future::Future, pin::Pin, sync::Arc};


use crate::bounds::{MaybeBoxFuture, MaybeSend, MaybeSync};
use futures::{future::Either, pin_mut};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::{
    AgentError, AgentEvent, AgentRunResult, AgentSpec, ArtifactKind, ArtifactRef, ProposalEnvelope,
    RedactionClassification, RunId, RunScope, RunWorkflow, ToolError, ToolSpec, TraceEvent,
    UserContext,
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionContext {
    pub run_id: RunId,
    pub agent_id: String,
    pub scope: RunScope,
    pub user: Option<UserContext>,
    #[serde(default)]
    pub metadata: Value,
}

pub type ToolContext = ExecutionContext;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactPublishRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(default)]
    pub kind: Option<ArtifactKind>,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default)]
    pub redaction_classification: Option<RedactionClassification>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SubagentRequest {
    pub agent_id: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RunScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<RunWorkflow>,
    #[serde(default)]
    pub metadata: Value,
}

pub type CancellationFuture<'a> = MaybeBoxFuture<'a, ()>;

pub trait CancellationSignal: MaybeSend + MaybeSync {
    fn is_cancelled(&self) -> bool;
    fn cancelled(&self) -> CancellationFuture<'_>;
}

#[derive(Clone)]
pub struct AgentCancellation {
    inner: Arc<dyn CancellationSignal>,
}

impl AgentCancellation {
    pub fn new(inner: Arc<dyn CancellationSignal>) -> Self {
        Self { inner }
    }

    pub fn none() -> Self {
        Self::new(Arc::new(NoopCancellation))
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    pub fn cancelled(&self) -> CancellationFuture<'_> {
        self.inner.cancelled()
    }
}

impl Default for AgentCancellation {
    fn default() -> Self {
        Self::none()
    }
}

struct NoopCancellation;

impl CancellationSignal for NoopCancellation {
    fn is_cancelled(&self) -> bool {
        false
    }

    fn cancelled(&self) -> CancellationFuture<'_> {
        Box::pin(std::future::pending())
    }
}

#[derive(Clone)]
pub struct AgentContext {
    pub run_id: RunId,
    pub now: OffsetDateTime,
    pub user: Option<UserContext>,
    pub scope: RunScope,
    pub input: Value,
    pub services: Arc<dyn AgentServices>,
    pub cancellation: AgentCancellation,
    pub trace: Arc<dyn TraceSink>,
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait::async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait::async_trait)]
pub trait Agent: MaybeSend + MaybeSync {
    fn spec(&self) -> AgentSpec;
    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError>;
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait::async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait::async_trait)]
pub trait ToolCaller: MaybeSend + MaybeSync {
    async fn call_tool(&self, name: &str, input: Value) -> Result<Value, ToolError>;

    async fn call_tool_with_cancellation(
        &self,
        name: &str,
        input: Value,
        cancellation: AgentCancellation,
    ) -> Result<Value, ToolError> {
        let tool_name = name.to_owned();
        let cancelled = cancellation.cancelled();
        let tool_call = self.call_tool(name, input);
        pin_mut!(cancelled);
        pin_mut!(tool_call);
        match futures::future::select(cancelled, tool_call).await {
            Either::Left(((), _)) => Err(ToolError::cancelled(format!(
                "tool '{tool_name}' cancelled"
            ))),
            Either::Right((result, _)) => result,
        }
    }
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait::async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait::async_trait)]
pub trait SubagentRunner: MaybeSend + MaybeSync {
    async fn run_subagent(&self, request: SubagentRequest) -> Result<Value, ToolError> {
        let _ = request;
        Err(ToolError::policy_denied(
            "subagent execution is not supported by this AgentServices implementation",
            serde_json::json!({"effect": "subagent"}),
        ))
    }

    async fn run_subagent_with_cancellation(
        &self,
        request: SubagentRequest,
        cancellation: AgentCancellation,
    ) -> Result<Value, ToolError> {
        let agent_id = request.agent_id.clone();
        let cancelled = cancellation.cancelled();
        let subagent_run = self.run_subagent(request);
        pin_mut!(cancelled);
        pin_mut!(subagent_run);
        match futures::future::select(cancelled, subagent_run).await {
            Either::Left(((), _)) => Err(ToolError::cancelled(format!(
                "subagent '{agent_id}' cancelled"
            ))),
            Either::Right((result, _)) => result,
        }
    }
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait::async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait::async_trait)]
pub trait AgentEventEmitter: MaybeSend + MaybeSync {
    async fn emit_event(&self, event: AgentEvent) -> Result<(), AgentError>;
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait::async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait::async_trait)]
pub trait AgentStateAccess: MaybeSend + MaybeSync {
    async fn load_state(&self, key: &str) -> Result<Option<Value>, AgentError>;
    async fn save_state(&self, key: &str, value: Value) -> Result<(), AgentError>;
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait::async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait::async_trait)]
pub trait ProposalCreator: MaybeSend + MaybeSync {
    async fn create_proposal(&self, proposal: ProposalEnvelope) -> Result<(), AgentError> {
        let _ = proposal;
        Err(AgentError::validation(
            "proposal creation is not supported by this AgentServices implementation",
        ))
    }
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait::async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait::async_trait)]
pub trait ArtifactPublisher: MaybeSend + MaybeSync {
    async fn publish_artifact(
        &self,
        request: ArtifactPublishRequest,
    ) -> Result<ArtifactRef, AgentError> {
        let _ = request;
        Err(AgentError::validation(
            "artifact publishing is not supported by this AgentServices implementation",
        ))
    }
}

pub trait AgentServices:
    ToolCaller
    + SubagentRunner
    + AgentEventEmitter
    + AgentStateAccess
    + ProposalCreator
    + ArtifactPublisher
{
}

impl<T> AgentServices for T where
    T: ToolCaller
        + SubagentRunner
        + AgentEventEmitter
        + AgentStateAccess
        + ProposalCreator
        + ArtifactPublisher
{
}

pub trait AgentServicesFactory: MaybeSend + MaybeSync {
    fn bind(&self, context: ExecutionContext) -> Arc<dyn AgentServices>;
}

pub struct StaticAgentServicesFactory {
    services: Arc<dyn AgentServices>,
}

impl StaticAgentServicesFactory {
    pub fn new(services: Arc<dyn AgentServices>) -> Self {
        Self { services }
    }
}

impl AgentServicesFactory for StaticAgentServicesFactory {
    fn bind(&self, _context: ExecutionContext) -> Arc<dyn AgentServices> {
        self.services.clone()
    }
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait::async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait::async_trait)]
pub trait TraceSink: MaybeSend + MaybeSync {
    async fn emit(&self, event: TraceEvent) -> Result<(), AgentError>;
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait::async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait::async_trait)]
pub trait ToolRegistry: MaybeSend + MaybeSync {
    async fn list_tools(&self) -> Result<Vec<ToolSpec>, ToolError>;
    async fn call(&self, name: &str, input: Value, ctx: ToolContext) -> Result<Value, ToolError>;
}
