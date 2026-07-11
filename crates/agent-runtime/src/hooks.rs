use std::{future::Future, pin::Pin, process::Stdio, sync::Arc, time::Duration};

use agent_core::{
    AgentError, HookEffect, HookEvent, HookEventName, HookInvocationStatus, HookKind, HookSpec,
    PROTOCOL_VERSION, PolicyDecision, PolicyDecisionKind, RunId, TraceEvent, TraceSink,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio::{io::AsyncWriteExt, process::Command as TokioCommand};

#[derive(Debug, Clone)]
pub struct HookInvocation {
    pub event: HookEventName,
    pub run_id: Option<RunId>,
    pub agent_id: Option<String>,
    pub input: Value,
}

#[async_trait]
pub trait HookHandler: Send + Sync {
    async fn handle(&self, invocation: HookInvocation) -> Result<Value, AgentError>;
}

pub struct FnHook<F> {
    f: F,
}

impl<F> FnHook<F> {
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

#[async_trait]
impl<F> HookHandler for FnHook<F>
where
    F: Send
        + Sync
        + 'static
        + Fn(HookInvocation) -> Pin<Box<dyn Future<Output = Result<Value, AgentError>> + Send>>,
{
    async fn handle(&self, invocation: HookInvocation) -> Result<Value, AgentError> {
        (self.f)(invocation).await
    }
}

#[derive(Clone)]
pub struct HookRegistration {
    spec: HookSpec,
    handler: Arc<dyn HookHandler>,
}

impl HookRegistration {
    pub fn native(spec: HookSpec, handler: Arc<dyn HookHandler>) -> Self {
        Self { spec, handler }
    }

    pub fn process(spec: HookSpec) -> Result<Self, AgentError> {
        let Some(command) = spec.command.clone() else {
            return Err(AgentError::validation(format!(
                "process hook '{}' requires a command",
                spec.name
            )));
        };
        let timeout = Duration::from_millis(spec.timeout_ms.unwrap_or(10_000).max(1));
        Ok(Self {
            spec,
            handler: Arc::new(ProcessHook { command, timeout }),
        })
    }
}

#[derive(Clone, Default)]
pub struct HookManager {
    hooks: Arc<Vec<HookRegistration>>,
}

impl HookManager {
    pub fn new(hooks: Vec<HookRegistration>) -> Self {
        Self {
            hooks: Arc::new(hooks),
        }
    }

    pub fn from_specs(specs: Vec<HookSpec>) -> Result<Self, AgentError> {
        let mut hooks = Vec::new();
        for spec in specs {
            if !spec.enabled {
                continue;
            }
            match spec.kind {
                HookKind::Process => hooks.push(HookRegistration::process(spec)?),
                HookKind::NativeRust | HookKind::Server => {
                    return Err(AgentError::validation(format!(
                        "hook '{}' requires a runtime handler for kind {:?}",
                        spec.name, spec.kind
                    )));
                }
            }
        }
        Ok(Self::new(hooks))
    }

    pub async fn observe(
        &self,
        event: HookEventName,
        run_id: Option<RunId>,
        agent_id: Option<String>,
        input: Value,
        trace: &dyn TraceSink,
    ) -> Result<(), AgentError> {
        for hook in self.matching(event, HookEffect::Observe) {
            let outcome = self
                .invoke(
                    hook,
                    event,
                    run_id.clone(),
                    agent_id.clone(),
                    input.clone(),
                    trace,
                )
                .await;
            if outcome.is_err() {
                continue;
            }
        }
        Ok(())
    }

    pub async fn authorize(
        &self,
        event: HookEventName,
        run_id: Option<RunId>,
        agent_id: Option<String>,
        input: Value,
        trace: &dyn TraceSink,
    ) -> Result<PolicyDecision, AgentError> {
        for hook in self.matching(event, HookEffect::Policy) {
            let output = self
                .invoke(
                    hook,
                    event,
                    run_id.clone(),
                    agent_id.clone(),
                    input.clone(),
                    trace,
                )
                .await?;
            let decision = parse_policy_decision(output)?;
            if decision.is_denied() {
                return Ok(decision);
            }
        }
        Ok(PolicyDecision::allow())
    }

    fn matching(&self, event: HookEventName, effect: HookEffect) -> Vec<&HookRegistration> {
        self.hooks
            .iter()
            .filter(|hook| {
                hook.spec.enabled && hook.spec.event == event && hook.spec.effect == effect
            })
            .collect()
    }

    async fn invoke(
        &self,
        hook: &HookRegistration,
        event: HookEventName,
        run_id: Option<RunId>,
        agent_id: Option<String>,
        input: Value,
        trace: &dyn TraceSink,
    ) -> Result<Value, AgentError> {
        let started_at = OffsetDateTime::now_utc();
        let timer = std::time::Instant::now();
        let invocation = HookInvocation {
            event,
            run_id: run_id.clone(),
            agent_id: agent_id.clone(),
            input: input.clone(),
        };
        let result = hook.handler.handle(invocation).await;
        let finished_at = OffsetDateTime::now_utc();
        let duration_ms = u64::try_from(timer.elapsed().as_millis()).unwrap_or(u64::MAX);
        let trace_input = auditable_hook_input(event, &input);
        let event_record = match &result {
            Ok(output) => HookEvent {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                hook_event: event,
                hook_kind: hook.spec.kind,
                hook_name: hook.spec.name.clone(),
                command: hook.spec.command.clone(),
                run_id,
                agent_id,
                status: HookInvocationStatus::Completed,
                started_at,
                finished_at,
                duration_ms,
                input: trace_input.clone(),
                output: Some(auditable_hook_output(event, output)),
                error: None,
            },
            Err(error) => HookEvent {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                hook_event: event,
                hook_kind: hook.spec.kind,
                hook_name: hook.spec.name.clone(),
                command: hook.spec.command.clone(),
                run_id,
                agent_id,
                status: HookInvocationStatus::Failed,
                started_at,
                finished_at,
                duration_ms,
                input: trace_input,
                output: None,
                error: Some(json!(error.record)),
            },
        };
        trace
            .emit(TraceEvent::new(
                "hook_invocation",
                serde_json::to_value(&event_record)
                    .map_err(|error| AgentError::internal(error.to_string()))?,
            ))
            .await?;
        result
    }
}

fn auditable_hook_input(event: HookEventName, input: &Value) -> Value {
    let mut input = input.clone();
    if matches!(
        event,
        HookEventName::BeforeToolCall
            | HookEventName::AfterToolCall
            | HookEventName::BeforeStateSave
            | HookEventName::AfterStateSave
    ) && let Some(object) = input.as_object_mut()
    {
        for field in ["input", "output", "value"] {
            object.remove(field);
        }
    }
    input
}

fn auditable_hook_output(event: HookEventName, output: &Value) -> Value {
    if matches!(
        event,
        HookEventName::BeforeToolCall
            | HookEventName::AfterToolCall
            | HookEventName::BeforeStateSave
            | HookEventName::AfterStateSave
    ) {
        return match output {
            Value::Object(object) => Value::Object(
                object
                    .iter()
                    .filter(|(key, _)| !matches!(key.as_str(), "input" | "output" | "value"))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            _ => json!({"redacted": true}),
        };
    }
    output.clone()
}

struct ProcessHook {
    command: Vec<String>,
    timeout: Duration,
}

#[async_trait]
impl HookHandler for ProcessHook {
    async fn handle(&self, invocation: HookInvocation) -> Result<Value, AgentError> {
        let Some((program, args)) = self.command.split_first() else {
            return Err(AgentError::validation(
                "process hook command cannot be empty",
            ));
        };
        let mut child = TokioCommand::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| AgentError::internal(format!("failed to spawn hook: {error}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            let input = serde_json::to_vec(&json!({
                "event": invocation.event,
                "run_id": invocation.run_id,
                "agent_id": invocation.agent_id,
                "input": invocation.input,
            }))
            .map_err(|error| AgentError::internal(error.to_string()))?;
            stdin.write_all(&input).await.map_err(|error| {
                AgentError::internal(format!("failed to write hook input: {error}"))
            })?;
        }
        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| AgentError::timeout(self.timeout))?
            .map_err(|error| AgentError::internal(format!("failed to wait for hook: {error}")))?;
        if !output.status.success() {
            return Err(AgentError::internal(format!(
                "hook exited with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        if output.stdout.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_slice(&output.stdout).or_else(|_| {
            Ok(json!({
                "stdout": String::from_utf8_lossy(&output.stdout).trim(),
            }))
        })
    }
}

fn parse_policy_decision(output: Value) -> Result<PolicyDecision, AgentError> {
    if output.is_null() || output == json!({}) {
        return Ok(PolicyDecision::allow());
    }
    let decision: PolicyDecision = serde_json::from_value(output)
        .map_err(|error| AgentError::validation(format!("invalid policy hook output: {error}")))?;
    match decision.decision {
        PolicyDecisionKind::Allow | PolicyDecisionKind::Deny => Ok(decision),
    }
}
