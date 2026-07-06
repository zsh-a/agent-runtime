use std::{future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;
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
pub struct ToolContext {
    pub run_id: RunId,
    pub agent_id: String,
    pub user: Option<UserContext>,
}

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

pub type CancellationFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub trait CancellationSignal: Send + Sync {
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

#[async_trait]
pub trait Agent: Send + Sync {
    fn spec(&self) -> AgentSpec;
    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError>;
}

#[async_trait]
pub trait ToolCaller: Send + Sync {
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

#[async_trait]
pub trait SubagentRunner: Send + Sync {
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

#[async_trait]
pub trait AgentEventEmitter: Send + Sync {
    async fn emit_event(&self, event: AgentEvent) -> Result<(), AgentError>;
}

#[async_trait]
pub trait AgentStateAccess: Send + Sync {
    async fn load_state(&self, key: &str) -> Result<Option<Value>, AgentError>;
    async fn save_state(&self, key: &str, value: Value) -> Result<(), AgentError>;
}

#[async_trait]
pub trait ProposalCreator: Send + Sync {
    async fn create_proposal(&self, proposal: ProposalEnvelope) -> Result<(), AgentError> {
        let _ = proposal;
        Err(AgentError::validation(
            "proposal creation is not supported by this AgentServices implementation",
        ))
    }
}

#[async_trait]
pub trait ArtifactPublisher: Send + Sync {
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

#[async_trait]
pub trait TraceSink: Send + Sync {
    async fn emit(&self, event: TraceEvent) -> Result<(), AgentError>;
}

#[async_trait]
pub trait ToolRegistry: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<ToolSpec>, ToolError>;
    async fn call(&self, name: &str, input: Value, ctx: ToolContext) -> Result<Value, ToolError>;
}
