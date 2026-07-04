use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

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

#[derive(Clone)]
pub struct AgentContext {
    pub run_id: RunId,
    pub now: OffsetDateTime,
    pub user: Option<UserContext>,
    pub scope: RunScope,
    pub input: Value,
    pub services: Arc<dyn AgentServices>,
    pub cancellation: CancellationToken,
    pub trace: Arc<dyn TraceSink>,
}

#[async_trait]
pub trait Agent: Send + Sync {
    fn spec(&self) -> AgentSpec;
    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError>;
}

#[async_trait]
pub trait AgentServices: Send + Sync {
    async fn call_tool(&self, name: &str, input: Value) -> Result<Value, ToolError>;
    async fn call_tool_with_cancellation(
        &self,
        name: &str,
        input: Value,
        cancellation: CancellationToken,
    ) -> Result<Value, ToolError> {
        tokio::select! {
            _ = cancellation.cancelled() => {
                Err(ToolError::cancelled(format!("tool '{name}' cancelled")))
            }
            result = self.call_tool(name, input) => result,
        }
    }

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
        cancellation: CancellationToken,
    ) -> Result<Value, ToolError> {
        let agent_id = request.agent_id.clone();
        tokio::select! {
            _ = cancellation.cancelled() => {
                Err(ToolError::cancelled(format!("subagent '{agent_id}' cancelled")))
            }
            result = self.run_subagent(request) => result,
        }
    }

    async fn emit_event(&self, event: AgentEvent) -> Result<(), AgentError>;
    async fn load_state(&self, key: &str) -> Result<Option<Value>, AgentError>;
    async fn save_state(&self, key: &str, value: Value) -> Result<(), AgentError>;
    async fn create_proposal(&self, proposal: ProposalEnvelope) -> Result<(), AgentError> {
        let _ = proposal;
        Err(AgentError::validation(
            "proposal creation is not supported by this AgentServices implementation",
        ))
    }
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

#[async_trait]
pub trait TraceSink: Send + Sync {
    async fn emit(&self, event: TraceEvent) -> Result<(), AgentError>;
}

#[async_trait]
pub trait ToolRegistry: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<ToolSpec>, ToolError>;
    async fn call(&self, name: &str, input: Value, ctx: ToolContext) -> Result<Value, ToolError>;
}
