use std::sync::Arc;

use agent_core::{
    AgentError, AgentRegistry, AgentRuntimeCatalog, AgentServices, AgentSpec, AgentTrace,
    PROTOCOL_VERSION, RunRequest, ToolError, ToolSpec,
};
use agent_llm::{LlmMessage, LlmRole};
use agent_runtime::{
    AGENT_RUN_TOOL_NAME, AgentRunToolContext, AgentRunner, RunControl, RunOutcome,
    call_agent_run_tool,
};
use agent_store::{FileLockStore, FileProposalStore, FileRunStore};
use async_trait::async_trait;
use camino::Utf8PathBuf;
use miette::{IntoDiagnostic, Result, miette};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{
    catalog::{read_catalog, registry_from_catalog},
    config::{execution_policy, hook_manager},
    registry::load_registry,
    tools::CliServices,
    trace_store::write_store_trace,
};

use super::{
    data::TuiOptions,
    policy::{TuiToolPolicy, TuiToolPolicyDecision},
    tool_inventory::chat_tools_from_catalog,
};

#[derive(Clone)]
pub(super) struct TuiRuntime {
    inner: Arc<TuiRuntimeInner>,
}

struct TuiRuntimeInner {
    options: TuiOptions,
    agent_specs: Vec<AgentSpec>,
    catalog: Option<AgentRuntimeCatalog>,
    tool_specs: Vec<ToolSpec>,
    services: Arc<CliServices>,
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
        let loaded = load_runtime_registry(options).await?;
        let proposal_store = Arc::new(
            FileProposalStore::new(options.store_path.clone())
                .await
                .into_diagnostic()?,
        );
        let services = Arc::new(CliServices::with_proposal_store(
            options.tool_overrides.clone(),
            proposal_store,
        ));
        let runner_services: Arc<dyn AgentServices> = services.clone();
        let store = Arc::new(
            FileRunStore::new(options.store_path.clone())
                .await
                .into_diagnostic()?,
        );
        let lock_store = Arc::new(
            FileLockStore::new(options.store_path.clone())
                .await
                .into_diagnostic()?,
        );
        let runner = AgentRunner::new(loaded.registry, store, runner_services)
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
                agent_specs: loaded.agent_specs,
                catalog: loaded.catalog,
                tool_specs: loaded.tool_specs,
                services,
                runner,
                policy: TuiToolPolicy::new(options.allow_high_risk_tools),
                cancellation,
            }),
        })
    }

    pub(super) fn tool_services(&self, parent_agent_id: Option<String>) -> Arc<dyn AgentServices> {
        Arc::new(TuiToolServices {
            runtime: self.inner.clone(),
            parent_agent_id,
        })
    }

    pub(super) async fn run_agent_once(
        &self,
        agent_id: &str,
        input: Value,
        input_mode: &str,
    ) -> Result<RunOutcome> {
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
                    trigger: agent_core::TriggerKind::Manual,
                    metadata: json!({
                        "source": "agent_tui",
                        "input_mode": input_mode,
                        "surface": "agent_tui"
                    }),
                },
                RunControl {
                    cancellation: self.inner.cancellation.clone(),
                    trace_events: None,
                },
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

    pub(super) fn default_agent_id(&self) -> Result<String> {
        self.inner
            .agent_specs
            .first()
            .map(|agent| agent.id.clone())
            .ok_or_else(|| miette!("no default agent is available"))
    }

    pub(super) fn chat_tools(&self) -> Vec<ToolSpec> {
        self.inner.tool_specs.clone()
    }

    pub(super) fn tool_policy_decision(&self, name: &str) -> TuiToolPolicyDecision {
        self.inner
            .policy
            .evaluate(self.inner.tool_specs.iter().find(|tool| tool.name == name))
    }

    pub(super) fn chat_request_messages(
        &self,
        agent_id: &str,
        messages: Vec<LlmMessage>,
    ) -> Vec<LlmMessage> {
        let Some(system_prompt) = self.catalog_system_prompt(agent_id) else {
            return messages;
        };
        let mut request_messages = Vec::with_capacity(messages.len() + 1);
        request_messages.push(LlmMessage {
            role: LlmRole::System,
            content: Value::String(system_prompt),
            name: None,
            metadata: json!({"source": "agent_catalog"}),
        });
        request_messages.extend(messages);
        request_messages
    }

    async fn persist_trace(&self, trace: &AgentTrace) -> Result<()> {
        write_store_trace(&self.inner.options.store_path, trace).await
    }

    fn catalog_system_prompt(&self, agent_id: &str) -> Option<String> {
        let catalog = self.inner.catalog.as_ref()?;
        let agent = catalog.agents.iter().find(|agent| agent.id == agent_id)?;

        let mut sections = vec![format!("You are {}.", agent.name)];
        if let Some(description) = agent
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty())
        {
            sections.push(description.to_owned());
        }
        let mut prompt_blocks = catalog.prompt_blocks.clone();
        prompt_blocks.sort_by_key(|block| block.index);
        for block in prompt_blocks {
            let text = block.text.trim();
            if !text.is_empty() {
                sections.push(text.to_owned());
            }
        }
        if !catalog.tools.is_empty() {
            sections.push(
                "Use the provided tools when they are necessary. Keep normal replies concise and direct."
                    .to_owned(),
            );
        }
        Some(sections.join("\n\n"))
    }
}

struct LoadedRuntimeRegistry {
    registry: Arc<dyn AgentRegistry>,
    agent_specs: Vec<AgentSpec>,
    catalog: Option<AgentRuntimeCatalog>,
    tool_specs: Vec<ToolSpec>,
}

async fn load_runtime_registry(options: &TuiOptions) -> Result<LoadedRuntimeRegistry> {
    match &options.catalog_path {
        Some(path) => {
            let catalog = read_catalog(path.clone()).await?;
            let registry: Arc<dyn AgentRegistry> = registry_from_catalog(&catalog);
            let agent_specs = catalog.agents.clone();
            let tool_specs = chat_tools_from_catalog(Some(&catalog), options);
            Ok(LoadedRuntimeRegistry {
                registry,
                agent_specs,
                catalog: Some(catalog),
                tool_specs,
            })
        }
        None => {
            let registry_config = load_registry(options.registry_path.clone()).await?;
            let agent_specs = registry_config.list_specs();
            let registry = registry_config.into_agent_registry();
            let registry: Arc<dyn AgentRegistry> = registry;
            let tool_specs = chat_tools_from_catalog(None, options);
            Ok(LoadedRuntimeRegistry {
                registry,
                agent_specs,
                catalog: None,
                tool_specs,
            })
        }
    }
}

struct TuiToolServices {
    runtime: Arc<TuiRuntimeInner>,
    parent_agent_id: Option<String>,
}

#[async_trait]
impl AgentServices for TuiToolServices {
    async fn call_tool(&self, name: &str, input: Value) -> std::result::Result<Value, ToolError> {
        self.call_tool_with_cancellation(name, input, self.runtime.cancellation.clone())
            .await
    }

    async fn call_tool_with_cancellation(
        &self,
        name: &str,
        input: Value,
        cancellation: CancellationToken,
    ) -> std::result::Result<Value, ToolError> {
        let decision = self.runtime.policy.evaluate(
            self.runtime
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
        if name != AGENT_RUN_TOOL_NAME {
            return tokio::select! {
                _ = cancellation.cancelled() => {
                    Err(ToolError::cancelled(format!("tool '{name}' cancelled")))
                }
                result = self.runtime.services.call_tool(name, input) => result,
            };
        }
        let output = call_agent_run_tool(
            &self.runtime.runner,
            input,
            AgentRunToolContext {
                parent_agent_id: self.parent_agent_id.clone(),
                metadata: json!({
                    "source": "agent_tui",
                    "surface": "agent_tui",
                }),
                cancellation,
                ..AgentRunToolContext::default()
            },
        )
        .await?;
        persist_agent_run_trace(&self.runtime.options.store_path, &output).await?;
        Ok(output)
    }

    async fn emit_event(
        &self,
        event: agent_core::AgentEvent,
    ) -> std::result::Result<(), AgentError> {
        self.runtime.services.emit_event(event).await
    }

    async fn load_state(&self, key: &str) -> std::result::Result<Option<Value>, AgentError> {
        self.runtime.services.load_state(key).await
    }

    async fn save_state(&self, key: &str, value: Value) -> std::result::Result<(), AgentError> {
        self.runtime.services.save_state(key, value).await
    }

    async fn create_proposal(
        &self,
        proposal: agent_core::ProposalEnvelope,
    ) -> std::result::Result<(), AgentError> {
        self.runtime.services.create_proposal(proposal).await
    }
}

async fn persist_agent_run_trace(
    store_path: &Utf8PathBuf,
    output: &Value,
) -> Result<(), ToolError> {
    let Some(trace) = output.get("trace").cloned() else {
        return Ok(());
    };
    let trace: AgentTrace = serde_json::from_value(trace).map_err(|error| {
        tool_internal_error(format!("failed to decode agent.run trace: {error}"))
    })?;
    write_store_trace(store_path, &trace)
        .await
        .map_err(|error| tool_internal_error(format!("failed to persist agent.run trace: {error}")))
}

fn tool_internal_error(message: impl Into<String>) -> ToolError {
    ToolError {
        record: AgentError::internal(message).record,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::policy::TuiToolRisk;
    use crate::tui::test_support::{catalog_options, test_options};
    use crate::tui::tool_inventory::load_tui_tool_inventory;
    use agent_core::{AgentRunStatus, ToolRisk};

    #[tokio::test]
    async fn chat_tools_include_agent_run() {
        let dir = tempfile::tempdir().expect("temp dir");
        let options = test_options(&dir, "mock response", true);

        let runtime = TuiRuntime::load(&options).await.expect("runtime loads");
        let tools = runtime.chat_tools();
        let decision = runtime.tool_policy_decision(AGENT_RUN_TOOL_NAME);

        assert!(tools.iter().any(|tool| tool.name == AGENT_RUN_TOOL_NAME));
        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert_eq!(decision.risk.label(), "high");
        assert!(decision.allowed);
    }

    #[tokio::test]
    async fn runtime_services_block_high_risk_when_policy_denies_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let options = test_options(&dir, "mock response", false);
        let runtime = TuiRuntime::load(&options).await.expect("runtime loads");

        let error = runtime
            .call_tool(
                AGENT_RUN_TOOL_NAME,
                json!({"agent_id":"echo_agent","input":{"message":"blocked"}}),
            )
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

    #[tokio::test]
    async fn runtime_agent_run_tool_uses_cancellation_token() {
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
                .call_tool(
                    AGENT_RUN_TOOL_NAME,
                    json!({
                        "agent_id": "customer_summary_agent",
                        "input": {"sleep_ms": 5000}
                    }),
                )
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancellation.cancel();
        let output = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("cancelled agent.run returns promptly")
            .expect("agent.run task joins")
            .expect("cancelled agent.run returns output");

        assert_eq!(output["result"]["status"], "cancelled");
        let trace = output["trace"]["events"]
            .as_array()
            .expect("trace events are present");
        assert!(trace.iter().any(|event| event["kind"] == "run_cancelled"));
    }
}
