use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use crate::{
    AgentError, AgentEvent, AgentRunResult, AgentSpec, ProposalEnvelope, RunId, ToolError,
    ToolSpec, TraceEvent, UserContext,
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolContext {
    pub run_id: RunId,
    pub agent_id: String,
    pub user: Option<UserContext>,
}

#[derive(Clone)]
pub struct AgentContext {
    pub run_id: RunId,
    pub now: OffsetDateTime,
    pub user: Option<UserContext>,
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
    async fn emit_event(&self, event: AgentEvent) -> Result<(), AgentError>;
    async fn load_state(&self, key: &str) -> Result<Option<Value>, AgentError>;
    async fn save_state(&self, key: &str, value: Value) -> Result<(), AgentError>;
    async fn create_proposal(&self, proposal: ProposalEnvelope) -> Result<(), AgentError> {
        let _ = proposal;
        Err(AgentError::validation(
            "proposal creation is not supported by this AgentServices implementation",
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
