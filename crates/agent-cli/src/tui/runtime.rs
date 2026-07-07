use std::sync::Arc;

use agent_core::{
    AgentCancellation, AgentError, AgentEvent, AgentEventEmitter, AgentRunStatus, AgentServices,
    AgentSpec, AgentStateAccess, AgentTrace, ArtifactPublisher, PROTOCOL_VERSION, ProposalCreator,
    RunId, RunRequest, SubagentRunner, ToolCaller, ToolError, ToolSpec, WorkflowRunRequest,
    WorkflowRunResult,
};
use agent_llm::LlmMessage;
use agent_runtime::{AgentRunner, RunControl, RunOutcome};
use agent_store::{FileLockStore, FileProposalStore, FileRunStore};
use async_trait::async_trait;
use miette::{IntoDiagnostic, Result, miette};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{
    cancellation::agent_cancellation,
    config::{execution_policy, hook_manager},
    runtime_config::{RuntimeComposition, RuntimeSourceOptions, compose_runtime_sources},
    tools::CliServices,
    trace_store::{write_store_trace, write_workflow_traces},
};

use super::{
    data::TuiOptions,
    policy::{TuiToolPolicy, TuiToolPolicyDecision},
};

#[derive(Clone)]
pub(super) struct TuiRuntime {
    inner: Arc<TuiRuntimeInner>,
}

struct TuiRuntimeInner {
    options: TuiOptions,
    composition: RuntimeComposition,
    services: Arc<CliServices>,
    run_store: Arc<dyn agent_core::AgentRunStore>,
    runner: AgentRunner,
    policy: TuiToolPolicy,
    cancellation: CancellationToken,
}

impl TuiRuntime {
    pub(super) async fn load(options: &TuiOptions) -> Result<Self> {
        Self::load_with_cancellation(options, CancellationToken::new()).await
    }

    pub(super) async fn load_with_cancellation(
        options: &TuiOptions,
        cancellation: CancellationToken,
    ) -> Result<Self> {
        let composition = compose_runtime_sources(RuntimeSourceOptions {
            sources: options.runtime_sources.clone(),
            tool_overrides: options.tool_overrides.clone(),
        })
        .await?;
        let proposal_store = Arc::new(
            FileProposalStore::new(options.store_path.clone())
                .await
                .into_diagnostic()?,
        );
        let mut tool_overrides = options.tool_overrides.clone();
        tool_overrides.extend_tool_specs(composition.tool_specs.clone());
        let services = Arc::new(CliServices::with_proposal_store(
            tool_overrides,
            proposal_store,
        ));
        let runner_services: Arc<dyn AgentServices> = services.clone();
        let store = Arc::new(
            FileRunStore::new(options.store_path.clone())
                .await
                .into_diagnostic()?,
        );
        let run_store: Arc<dyn agent_core::AgentRunStore> = store.clone();
        let lock_store = Arc::new(
            FileLockStore::new(options.store_path.clone())
                .await
                .into_diagnostic()?,
        );
        let runner = AgentRunner::new(
            composition.registry.clone(),
            run_store.clone(),
            runner_services,
        )
        .with_lock_store(lock_store)
        .with_hooks(hook_manager(options.hooks.clone())?)
        .with_policy(execution_policy(
            options.timeout_seconds,
            options.max_retries,
            options.retry_backoff_ms,
        ));

        Ok(Self {
            inner: Arc::new(TuiRuntimeInner {
                options: options.clone(),
                composition,
                services,
                run_store,
                runner,
                policy: TuiToolPolicy::new(options.allow_high_risk_tools),
                cancellation,
            }),
        })
    }

    pub(super) fn tool_services(&self, parent_agent_id: Option<String>) -> Arc<dyn AgentServices> {
        let _ = parent_agent_id;
        Arc::new(TuiToolServices {
            runtime: self.inner.clone(),
        })
    }

    pub(super) async fn run_agent_once(
        &self,
        agent_id: &str,
        input: Value,
        input_mode: &str,
    ) -> Result<RunOutcome> {
        let mut control = RunControl::default();
        control.cancellation = self.inner.cancellation.clone();
        let outcome = self
            .inner
            .runner
            .run_once_with_control(
                agent_id,
                RunRequest {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    run_id: None,
                    input,
                    user: None,
                    scope: None,
                    trigger: agent_core::TriggerKind::Manual,
                    trigger_envelope: None,
                    workflow: None,
                    metadata: json!({
                        "source": "agent_tui",
                        "input_mode": input_mode,
                        "surface": "agent_tui"
                    }),
                },
                control,
            )
            .await
            .into_diagnostic()?;
        self.persist_trace(&outcome.trace).await?;
        Ok(outcome)
    }

    pub(super) async fn call_tool(&self, name: &str, input: Value) -> Result<Value> {
        self.tool_services(None)
            .call_tool(name, input)
            .await
            .map_err(|err| miette!(err.record.message))
    }

    pub(super) async fn run_workflow(
        &self,
        request: WorkflowRunRequest,
    ) -> Result<WorkflowRunResult> {
        let result = self
            .inner
            .runner
            .run_workflow(request)
            .await
            .into_diagnostic()?;
        write_workflow_traces(&self.inner.options.store_path, &result).await?;
        Ok(result)
    }

    pub(super) async fn cancel_run(&self, run_id: RunId) -> Result<TuiCancelRunResult> {
        let Some(mut run) = self
            .inner
            .run_store
            .get_run(&run_id)
            .await
            .into_diagnostic()?
        else {
            return Err(miette!("run '{}' was not found", run_id.0));
        };
        let status = run.status.clone();
        if status == AgentRunStatus::Running {
            run.request_cancellation(
                time::OffsetDateTime::now_utc(),
                Some("agent_tui".to_owned()),
            );
            self.inner
                .run_store
                .update_run(run)
                .await
                .into_diagnostic()?;
            return Ok(TuiCancelRunResult {
                run_id,
                cancellation_requested: true,
                status,
                message: "cancellation intent persisted".to_owned(),
            });
        }
        Ok(TuiCancelRunResult {
            run_id,
            cancellation_requested: false,
            status,
            message: "run is not active".to_owned(),
        })
    }

    pub(super) fn default_agent_id(&self) -> Result<String> {
        self.inner
            .composition
            .agent_specs
            .first()
            .map(|agent| agent.id.clone())
            .ok_or_else(|| miette!("no default agent is available"))
    }

    pub(super) fn resolve_agent_id(&self, selected_agent_id: Option<&str>) -> Result<String> {
        let Some(selected_agent_id) = selected_agent_id
            .map(str::trim)
            .filter(|agent_id| !agent_id.is_empty())
        else {
            return self.default_agent_id();
        };
        if self
            .inner
            .composition
            .agent_specs
            .iter()
            .any(|agent| agent.id == selected_agent_id)
        {
            return Ok(selected_agent_id.to_owned());
        }
        Err(miette!("unknown agent '{selected_agent_id}'"))
    }

    pub(super) fn agent_specs(&self) -> &[AgentSpec] {
        &self.inner.composition.agent_specs
    }

    pub(super) fn chat_tools(&self) -> Vec<ToolSpec> {
        self.inner.composition.tool_specs.clone()
    }

    pub(super) fn tool_policy_decision(&self, name: &str) -> TuiToolPolicyDecision {
        self.inner.policy.evaluate(
            self.inner
                .composition
                .tool_specs
                .iter()
                .find(|tool| tool.name == name),
        )
    }

    pub(super) fn chat_request_messages(
        &self,
        agent_id: &str,
        messages: Vec<LlmMessage>,
    ) -> Vec<LlmMessage> {
        self.inner.composition.chat_messages(agent_id, messages)
    }

    async fn persist_trace(&self, trace: &AgentTrace) -> Result<()> {
        write_store_trace(&self.inner.options.store_path, trace).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiCancelRunResult {
    pub(super) run_id: RunId,
    pub(super) cancellation_requested: bool,
    pub(super) status: AgentRunStatus,
    pub(super) message: String,
}

struct TuiToolServices {
    runtime: Arc<TuiRuntimeInner>,
}

#[async_trait]
impl ToolCaller for TuiToolServices {
    async fn call_tool(&self, name: &str, input: Value) -> std::result::Result<Value, ToolError> {
        let cancellation = agent_cancellation(self.runtime.cancellation.clone());
        ToolCaller::call_tool_with_cancellation(self, name, input, cancellation).await
    }

    async fn call_tool_with_cancellation(
        &self,
        name: &str,
        input: Value,
        cancellation: AgentCancellation,
    ) -> std::result::Result<Value, ToolError> {
        let decision = self.runtime.policy.evaluate(
            self.runtime
                .composition
                .tool_specs
                .iter()
                .find(|tool| tool.name == name),
        );
        if !decision.allowed {
            return Err(ToolError::policy_denied(
                format!("tool '{name}' is blocked by the current TUI tool policy"),
                json!({
                    "tool_name": name,
                    "risk": decision.risk.label(),
                    "surface": "agent_tui",
                }),
            ));
        }
        tokio::select! {
            _ = cancellation.cancelled() => {
                Err(ToolError::cancelled(format!("tool '{name}' cancelled")))
            }
            result = ToolCaller::call_tool(self.runtime.services.as_ref(), name, input) => result,
        }
    }
}

#[async_trait]
impl AgentEventEmitter for TuiToolServices {
    async fn emit_event(&self, event: AgentEvent) -> std::result::Result<(), AgentError> {
        AgentEventEmitter::emit_event(self.runtime.services.as_ref(), event).await
    }
}

#[async_trait]
impl AgentStateAccess for TuiToolServices {
    async fn load_state(&self, key: &str) -> std::result::Result<Option<Value>, AgentError> {
        AgentStateAccess::load_state(self.runtime.services.as_ref(), key).await
    }

    async fn save_state(&self, key: &str, value: Value) -> std::result::Result<(), AgentError> {
        AgentStateAccess::save_state(self.runtime.services.as_ref(), key, value).await
    }
}

#[async_trait]
impl ProposalCreator for TuiToolServices {
    async fn create_proposal(
        &self,
        proposal: agent_core::ProposalEnvelope,
    ) -> std::result::Result<(), AgentError> {
        ProposalCreator::create_proposal(self.runtime.services.as_ref(), proposal).await
    }
}

#[async_trait]
impl SubagentRunner for TuiToolServices {}

#[async_trait]
impl ArtifactPublisher for TuiToolServices {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::policy::TuiToolRisk;
    use crate::tui::test_support::{catalog_options, test_options};
    use crate::tui::tool_inventory::load_tui_tool_inventory;
    use agent_core::{AgentRunStatus, ToolRisk};
    use camino::Utf8PathBuf;

    #[tokio::test]
    async fn chat_tools_exclude_agent_run() {
        let dir = tempfile::tempdir().expect("temp dir");
        let options = test_options(&dir, "mock response", true);

        let runtime = TuiRuntime::load(&options).await.expect("runtime loads");
        let tools = runtime.chat_tools();

        assert!(!tools.iter().any(|tool| tool.name == "agent.run"));
        assert!(tools.iter().any(|tool| tool.name == "echo"));
    }

    #[tokio::test]
    async fn runtime_services_block_high_risk_when_policy_denies_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let options = test_options(&dir, "mock response", false);
        let runtime = TuiRuntime::load(&options).await.expect("runtime loads");

        let error = runtime
            .call_tool("uncontracted.tool", json!({"message":"blocked"}))
            .await
            .expect_err("runtime policy blocks high-risk tools");

        assert!(
            error
                .to_string()
                .contains("blocked by the current TUI tool policy")
        );

        let missing = runtime.tool_policy_decision("uncontracted.tool");
        assert_eq!(missing.risk, TuiToolRisk::High);
        assert!(!missing.allowed);
    }

    #[tokio::test]
    async fn runtime_policy_uses_tool_source_risk_for_shell_exec() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut options = test_options(&dir, "mock response", false);
        options.tool_overrides.source_specs.push(ToolSpec {
            name: "shell.exec".to_owned(),
            description: "Execute a shell command.".to_owned(),
            input_schema: json!({"type": "object"}),
            output_schema: Some(json!({"type": "object"})),
            risk: ToolRisk::High,
            metadata: json!({"source": "test"}),
        });
        let runtime = TuiRuntime::load(&options).await.expect("runtime loads");

        let decision = runtime.tool_policy_decision("shell.exec");
        assert_eq!(decision.risk, TuiToolRisk::High);
        assert!(!decision.allowed);

        let inventory = load_tui_tool_inventory(&options)
            .await
            .expect("inventory loads");
        let shell = inventory
            .items
            .iter()
            .find(|item| item.name == "shell.exec")
            .expect("shell.exec inventory item is present");
        assert_eq!(shell.source, "test");
        assert_eq!(shell.risk, TuiToolRisk::High);
        assert!(!shell.allowed);
    }

    #[tokio::test]
    async fn runtime_run_agent_uses_cancellation_token() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cancellation = CancellationToken::new();
        let options = catalog_options(
            &dir,
            "mock response",
            Utf8PathBuf::from("../../examples/business-integration/catalog.json"),
        );
        let runtime = TuiRuntime::load_with_cancellation(&options, cancellation.clone())
            .await
            .expect("runtime loads");
        let run = tokio::spawn(async move {
            runtime
                .run_agent_once("customer_summary_agent", json!({"sleep_ms": 5000}), "test")
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancellation.cancel();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("cancelled run returns promptly")
            .expect("run task joins")
            .expect("cancelled run returns an outcome");

        assert_eq!(outcome.result.status, AgentRunStatus::Cancelled);
        assert!(
            outcome
                .trace
                .events
                .iter()
                .any(|event| event.kind == "run_cancelled")
        );
    }
}
