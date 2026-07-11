use std::sync::Arc;

use agent_core::{
    AgentCancellation, AgentError, AgentEvent, AgentEventEmitter, AgentServices, AgentStateAccess,
    AgentStateStore, ArtifactPublishRequest, ArtifactPublisher, HookEventName, ProposalCreator,
    ProposalEnvelope, RunId, RunScope, RunWorkflow, SubagentRequest, SubagentRunner, ToolCaller,
    ToolContext, ToolError, ToolRegistry, TraceEvent, TraceSink, UserContext,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::{SubagentRunContext, hooks::HookManager, run_subagent, runner::AgentRunner};

pub fn guarded_services(
    inner: Arc<dyn AgentServices>,
    context: agent_core::ExecutionContext,
    hooks: HookManager,
    cancellation: CancellationToken,
) -> Arc<dyn AgentServices> {
    Arc::new(TracedAgentServices {
        inner,
        trace: Arc::new(crate::MemoryTraceSink::default()),
        run_id: context.run_id,
        agent_id: context.agent_id,
        user: context.user,
        scope: context.scope,
        hooks,
        subagent_runner: None,
        cancellation,
        workflow: None,
    })
}

pub struct BasicAgentServices {
    agent_id: String,
    run_id: RunId,
    user: Option<UserContext>,
    scope: RunScope,
    tools: Arc<dyn ToolRegistry>,
    state_store: Arc<dyn AgentStateStore>,
}

pub(crate) struct TracedAgentServices {
    pub(crate) inner: Arc<dyn AgentServices>,
    pub(crate) trace: Arc<dyn TraceSink>,
    pub(crate) run_id: RunId,
    pub(crate) agent_id: String,
    pub(crate) user: Option<UserContext>,
    pub(crate) scope: RunScope,
    pub(crate) hooks: HookManager,
    pub(crate) subagent_runner: Option<AgentRunner>,
    pub(crate) cancellation: CancellationToken,
    pub(crate) workflow: Option<RunWorkflow>,
}

impl BasicAgentServices {
    pub fn new(
        agent_id: impl Into<String>,
        run_id: RunId,
        user: Option<UserContext>,
        scope: RunScope,
        tools: Arc<dyn ToolRegistry>,
        state_store: Arc<dyn AgentStateStore>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            run_id,
            user,
            scope,
            tools,
            state_store,
        }
    }
}

#[async_trait]
impl ToolCaller for BasicAgentServices {
    async fn call_tool(&self, name: &str, input: Value) -> Result<Value, ToolError> {
        self.tools
            .call(
                name,
                input,
                ToolContext {
                    run_id: self.run_id.clone(),
                    agent_id: self.agent_id.clone(),
                    scope: self.scope.clone(),
                    user: self.user.clone(),
                    metadata: json!({}),
                },
            )
            .await
    }
}

#[async_trait]
impl AgentEventEmitter for BasicAgentServices {
    async fn emit_event(&self, _event: AgentEvent) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait]
impl AgentStateAccess for BasicAgentServices {
    async fn load_state(&self, key: &str) -> Result<Option<Value>, AgentError> {
        self.state_store
            .load(&self.agent_id, &self.scope, key)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }

    async fn save_state(&self, key: &str, value: Value) -> Result<(), AgentError> {
        self.state_store
            .save(&self.agent_id, &self.scope, key, value)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }
}

#[async_trait]
impl SubagentRunner for BasicAgentServices {}

#[async_trait]
impl ProposalCreator for BasicAgentServices {}

#[async_trait]
impl ArtifactPublisher for BasicAgentServices {}

#[async_trait]
impl ToolCaller for TracedAgentServices {
    async fn call_tool(&self, name: &str, input: Value) -> Result<Value, ToolError> {
        let started_at = std::time::Instant::now();
        let input_hash = state_value_hash(&input);
        let input_bytes = serialized_value_len(&input);
        let policy_input = json!({
            "run_id": self.run_id.0.clone(),
            "agent_id": self.agent_id.clone(),
            "tool_name": name,
            "input": input.clone(),
        });
        let decision = self
            .hooks
            .authorize(
                HookEventName::BeforeToolCall,
                Some(self.run_id.clone()),
                Some(self.agent_id.clone()),
                policy_input.clone(),
                self.trace.as_ref(),
            )
            .await
            .map_err(ToolError::from_agent_error)?;
        if decision.is_denied() {
            return Err(ToolError::policy_denied(
                decision
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("tool call '{name}' denied by policy hook")),
                json!({
                    "decision": decision,
                    "event": "BeforeToolCall",
                    "tool_name": name,
                }),
            ));
        }
        self.hooks
            .observe(
                HookEventName::BeforeToolCall,
                Some(self.run_id.clone()),
                Some(self.agent_id.clone()),
                policy_input,
                self.trace.as_ref(),
            )
            .await
            .map_err(ToolError::from_agent_error)?;
        info!(
            run_id = %self.run_id.0,
            agent_id = %self.agent_id,
            tool_name = name,
            input_hash,
            input_bytes,
            "calling tool",
        );
        match self.inner.call_tool(name, input).await {
            Ok(output) => {
                let output_hash = state_value_hash(&output);
                let output_bytes = serialized_value_len(&output);
                let duration_ms = started_at.elapsed().as_millis();
                self.trace
                    .emit(TraceEvent::new(
                        "tool_call",
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "tool_name": name,
                            "duration_ms": duration_ms,
                            "status": "completed",
                            "input_hash": input_hash,
                            "input_bytes": input_bytes,
                            "output_hash": output_hash.clone(),
                            "output_bytes": output_bytes,
                        }),
                    ))
                    .await
                    .map_err(|error| ToolError {
                        record: error.record,
                    })?;
                info!(
                    run_id = %self.run_id.0,
                    agent_id = %self.agent_id,
                    tool_name = name,
                    output_hash,
                    output_bytes,
                    duration_ms,
                    "tool call completed",
                );
                self.hooks
                    .observe(
                        HookEventName::AfterToolCall,
                        Some(self.run_id.clone()),
                        Some(self.agent_id.clone()),
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "tool_name": name,
                            "status": "completed",
                            "output_hash": output_hash,
                            "output_bytes": output_bytes,
                            "duration_ms": duration_ms,
                        }),
                        self.trace.as_ref(),
                    )
                    .await
                    .map_err(ToolError::from_agent_error)?;
                Ok(output)
            }
            Err(error) => {
                let duration_ms = started_at.elapsed().as_millis();
                self.trace
                    .emit(TraceEvent::new(
                        "tool_call_failed",
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "tool_name": name,
                            "duration_ms": duration_ms,
                            "status": "failed",
                            "input_hash": input_hash,
                            "input_bytes": input_bytes,
                            "error": error.record.clone(),
                        }),
                    ))
                    .await
                    .map_err(|trace_error| ToolError {
                        record: trace_error.record,
                    })?;
                warn!(
                    run_id = %self.run_id.0,
                    agent_id = %self.agent_id,
                    tool_name = name,
                    error_code = %error.record.code,
                    error_kind = ?error.record.kind,
                    retryable = error.record.retryable,
                    duration_ms,
                    "tool call failed",
                );
                self.hooks
                    .observe(
                        HookEventName::AfterToolCall,
                        Some(self.run_id.clone()),
                        Some(self.agent_id.clone()),
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "tool_name": name,
                            "status": "failed",
                            "error": error.record.clone(),
                            "duration_ms": duration_ms,
                        }),
                        self.trace.as_ref(),
                    )
                    .await
                    .map_err(ToolError::from_agent_error)?;
                Err(error)
            }
        }
    }

    async fn call_tool_with_cancellation(
        &self,
        name: &str,
        input: Value,
        cancellation: AgentCancellation,
    ) -> Result<Value, ToolError> {
        tokio::select! {
            _ = cancellation.cancelled() => {
                Err(ToolError::cancelled(format!("tool '{name}' cancelled")))
            }
            result = self.call_tool(name, input) => result,
        }
    }
}

#[async_trait]
impl SubagentRunner for TracedAgentServices {
    async fn run_subagent(&self, request: SubagentRequest) -> Result<Value, ToolError> {
        self.run_subagent_with_cancellation(
            request,
            crate::cancellation::agent_cancellation(self.cancellation.clone()),
        )
        .await
    }

    async fn run_subagent_with_cancellation(
        &self,
        request: SubagentRequest,
        cancellation: AgentCancellation,
    ) -> Result<Value, ToolError> {
        let Some(runner) = &self.subagent_runner else {
            return Err(ToolError::policy_denied(
                "subagent execution is not available outside an AgentRunner",
                json!({"effect": "subagent"}),
            ));
        };
        run_subagent(
            runner,
            request,
            SubagentRunContext {
                parent_run_id: Some(self.run_id.clone()),
                parent_agent_id: Some(self.agent_id.clone()),
                user: self.user.clone(),
                scope: Some(self.scope.clone()),
                metadata: json!({}),
                trace: Some(self.trace.clone()),
                hooks: self.hooks.clone(),
                cancellation: bridge_cancellation_to_tokio(self.cancellation.clone(), cancellation),
                workflow: self.workflow.clone(),
            },
        )
        .await
    }
}

#[async_trait]
impl AgentEventEmitter for TracedAgentServices {
    async fn emit_event(&self, event: AgentEvent) -> Result<(), AgentError> {
        debug!(
            run_id = %self.run_id.0,
            agent_id = %self.agent_id,
            event_kind = %event.kind,
            "agent emitted event",
        );
        self.trace
            .emit(TraceEvent {
                kind: event.kind.clone(),
                occurred_at: event.occurred_at,
                payload: trace_agent_event_payload(
                    event.payload.clone(),
                    &self.run_id,
                    &self.agent_id,
                ),
            })
            .await?;
        self.inner.emit_event(event).await
    }
}

#[async_trait]
impl AgentStateAccess for TracedAgentServices {
    async fn load_state(&self, key: &str) -> Result<Option<Value>, AgentError> {
        let started_at = std::time::Instant::now();
        match self.inner.load_state(key).await {
            Ok(value) => {
                let mut payload = json!({
                    "run_id": self.run_id.0.clone(),
                    "agent_id": self.agent_id.clone(),
                    "key": key,
                    "duration_ms": started_at.elapsed().as_millis(),
                    "status": "completed",
                    "found": value.is_some(),
                });
                if let Some(value) = &value {
                    payload["value_hash"] = json!(state_value_hash(value));
                }
                self.trace
                    .emit(TraceEvent::new("state_read", payload))
                    .await?;
                Ok(value)
            }
            Err(error) => {
                self.trace
                    .emit(TraceEvent::new(
                        "state_read_failed",
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "key": key,
                            "duration_ms": started_at.elapsed().as_millis(),
                            "status": "failed",
                            "error": error.record.clone(),
                        }),
                    ))
                    .await?;
                Err(error)
            }
        }
    }

    async fn save_state(&self, key: &str, value: Value) -> Result<(), AgentError> {
        let started_at = std::time::Instant::now();
        let value_hash = state_value_hash(&value);
        let policy_input = json!({
            "run_id": self.run_id.0.clone(),
            "agent_id": self.agent_id.clone(),
            "key": key,
            "value": value.clone(),
            "value_hash": value_hash,
        });
        let decision = self
            .hooks
            .authorize(
                HookEventName::BeforeStateSave,
                Some(self.run_id.clone()),
                Some(self.agent_id.clone()),
                policy_input.clone(),
                self.trace.as_ref(),
            )
            .await?;
        if decision.is_denied() {
            return Err(AgentError::policy_denied(
                decision
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("state save '{key}' denied by policy hook")),
                json!({
                    "decision": decision,
                    "event": "BeforeStateSave",
                    "key": key,
                }),
            ));
        }
        self.hooks
            .observe(
                HookEventName::BeforeStateSave,
                Some(self.run_id.clone()),
                Some(self.agent_id.clone()),
                policy_input,
                self.trace.as_ref(),
            )
            .await?;
        match self.inner.save_state(key, value.clone()).await {
            Ok(()) => {
                self.trace
                    .emit(TraceEvent::new(
                        "state_write",
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "key": key,
                            "duration_ms": started_at.elapsed().as_millis(),
                            "status": "completed",
                            "value_hash": value_hash,
                        }),
                    ))
                    .await?;
                self.hooks
                    .observe(
                        HookEventName::AfterStateSave,
                        Some(self.run_id.clone()),
                        Some(self.agent_id.clone()),
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "key": key,
                            "status": "completed",
                            "value_hash": value_hash,
                            "duration_ms": started_at.elapsed().as_millis(),
                        }),
                        self.trace.as_ref(),
                    )
                    .await?;
                Ok(())
            }
            Err(error) => {
                self.trace
                    .emit(TraceEvent::new(
                        "state_write_failed",
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "key": key,
                            "duration_ms": started_at.elapsed().as_millis(),
                            "status": "failed",
                            "value_hash": value_hash,
                            "error": error.record.clone(),
                        }),
                    ))
                    .await?;
                self.hooks
                    .observe(
                        HookEventName::AfterStateSave,
                        Some(self.run_id.clone()),
                        Some(self.agent_id.clone()),
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "key": key,
                            "status": "failed",
                            "value_hash": value_hash,
                            "error": error.record.clone(),
                            "duration_ms": started_at.elapsed().as_millis(),
                        }),
                        self.trace.as_ref(),
                    )
                    .await?;
                Err(error)
            }
        }
    }
}

#[async_trait]
impl ProposalCreator for TracedAgentServices {
    async fn create_proposal(&self, proposal: ProposalEnvelope) -> Result<(), AgentError> {
        let started_at = std::time::Instant::now();
        let policy_input = json!({
            "run_id": self.run_id.0.clone(),
            "agent_id": self.agent_id.clone(),
            "proposal": proposal.clone(),
        });
        let decision = self
            .hooks
            .authorize(
                HookEventName::BeforeProposalCreate,
                Some(self.run_id.clone()),
                Some(self.agent_id.clone()),
                policy_input.clone(),
                self.trace.as_ref(),
            )
            .await?;
        if decision.is_denied() {
            return Err(AgentError::policy_denied(
                decision
                    .reason
                    .clone()
                    .unwrap_or_else(|| "proposal creation denied by policy hook".to_owned()),
                json!({
                    "decision": decision,
                    "event": "BeforeProposalCreate",
                    "proposal_id": proposal.proposal_id.0,
                }),
            ));
        }
        self.hooks
            .observe(
                HookEventName::BeforeProposalCreate,
                Some(self.run_id.clone()),
                Some(self.agent_id.clone()),
                policy_input,
                self.trace.as_ref(),
            )
            .await?;
        match self.inner.create_proposal(proposal.clone()).await {
            Ok(()) => {
                self.trace
                    .emit(TraceEvent::new(
                        "proposal_created",
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "proposal_id": proposal.proposal_id.0,
                            "kind": proposal.kind,
                            "summary": proposal.summary,
                            "status": proposal.status,
                            "duration_ms": started_at.elapsed().as_millis(),
                        }),
                    ))
                    .await?;
                self.hooks
                    .observe(
                        HookEventName::AfterProposalDecision,
                        Some(self.run_id.clone()),
                        Some(self.agent_id.clone()),
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "proposal_id": proposal.proposal_id.0,
                            "status": "completed",
                            "duration_ms": started_at.elapsed().as_millis(),
                        }),
                        self.trace.as_ref(),
                    )
                    .await?;
                Ok(())
            }
            Err(error) => {
                self.trace
                    .emit(TraceEvent::new(
                        "proposal_create_failed",
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "kind": proposal.kind,
                            "summary": proposal.summary,
                            "duration_ms": started_at.elapsed().as_millis(),
                            "error": error.record.clone(),
                        }),
                    ))
                    .await?;
                self.hooks
                    .observe(
                        HookEventName::AfterProposalDecision,
                        Some(self.run_id.clone()),
                        Some(self.agent_id.clone()),
                        json!({
                            "run_id": self.run_id.0.clone(),
                            "agent_id": self.agent_id.clone(),
                            "proposal_id": proposal.proposal_id.0,
                            "status": "failed",
                            "error": error.record.clone(),
                            "duration_ms": started_at.elapsed().as_millis(),
                        }),
                        self.trace.as_ref(),
                    )
                    .await?;
                Err(error)
            }
        }
    }
}

#[async_trait]
impl ArtifactPublisher for TracedAgentServices {
    async fn publish_artifact(
        &self,
        request: ArtifactPublishRequest,
    ) -> Result<agent_core::ArtifactRef, AgentError> {
        let started_at = std::time::Instant::now();
        let artifact = self.inner.publish_artifact(request).await?;
        self.trace
            .emit(TraceEvent::new(
                "artifact_published",
                json!({
                    "run_id": self.run_id.0.clone(),
                    "agent_id": self.agent_id.clone(),
                    "artifact_ref": artifact.clone(),
                    "duration_ms": started_at.elapsed().as_millis(),
                    "status": "completed",
                }),
            ))
            .await?;
        Ok(artifact)
    }
}

fn bridge_cancellation_to_tokio(
    parent: CancellationToken,
    cancellation: AgentCancellation,
) -> CancellationToken {
    if cancellation.is_cancelled() {
        parent.cancel();
        return parent;
    }
    let child = parent.child_token();
    let child_for_task = child.clone();
    tokio::spawn(async move {
        cancellation.cancelled().await;
        child_for_task.cancel();
    });
    child
}

fn state_value_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    format!("blake3:{}", blake3::hash(&bytes).to_hex())
}

fn serialized_value_len(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

fn trace_agent_event_payload(payload: Value, run_id: &RunId, agent_id: &str) -> Value {
    let mut payload = match payload {
        Value::Object(_) => payload,
        other => json!({ "value": other }),
    };
    if let Some(object) = payload.as_object_mut() {
        object
            .entry("run_id".to_owned())
            .or_insert_with(|| json!(run_id.0.clone()));
        object
            .entry("agent_id".to_owned())
            .or_insert_with(|| json!(agent_id));
    }
    payload
}
