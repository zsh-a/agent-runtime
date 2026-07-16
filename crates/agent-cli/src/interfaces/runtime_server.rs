use std::{collections::HashMap, sync::Arc};

use agent_chat::{ChatEventStream, ChatResumeRequest, ChatTurnRequest, ChatTurnRunner};
use agent_core::{
    AgentProposalStore, AgentRunEventStore, AgentRunRecord, AgentRunResult, AgentRunStatus,
    AgentRunStore, AgentRuntimeCatalog, AgentServices, AgentServicesFactory, AgentSessionStore,
    AgentTraceStore, ApprovalLevel, ContextPolicy, ExecutionContext, PROTOCOL_VERSION,
    ProposalDiff, ProposalEnvelope, ProposalId, ProposalWarning, RunEventCursor, RunEventRecord,
    RunId, RunRequest, RunScope, RunWorkflow, SessionId, SessionRecord, ThreadId, ThreadRecord,
    ToolSpec, TraceEvent, TriggerEnvelope, TriggerKind, UserContext, WorkflowRunRequest,
    WorkflowRunResult,
};
use agent_runtime::{AgentRunner, HookManager, RunControl, TraceEventBuffer, guarded_services};
use camino::Utf8PathBuf;
use futures::StreamExt;
use miette::{IntoDiagnostic, Result, miette};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::{
    chat::{ChatLlmOptions, provider_from_options},
    config::RuntimeStoreBackend,
    metrics::{RuntimeMetricsSummary, build_metrics_summary},
    proposal::{
        ProposalAction, ProposalActionResponse, ProposalDecisionInput, ProposalDecisionResponse,
        append_proposal_action_trace_event, append_proposal_created_trace_event,
        append_proposal_decision_trace_event, authorize_proposal_apply_policy,
        authorize_proposal_create_policy, decide_proposal_with_store,
        execute_proposal_action_with_store, parse_approval_decision, proposal_action_tool,
        proposal_kind_spec,
    },
    replay::{ReplayExecutionReport, ReplayMode, replay_source_trace},
    runtime_config::{
        ResolvedRuntimeSources, RuntimeComposition, RuntimeSourceOptions, compose_runtime_sources,
    },
    runtime_stores::RuntimeStores,
    session::{
        HttpSessionCreateParams, HttpSessionCreateResponse, HttpThreadForkParams,
        SessionShowReport, ThreadForkReport, ThreadWithSteps, ensure_thread,
        record_chat_event_step, record_session_step,
    },
    tools::{CliServices, ToolOverrides},
};

mod chat;
mod construction;
mod proposals;
mod runs;
mod sessions;
mod types;

pub(crate) use types::*;

#[derive(Clone)]
pub(crate) struct RuntimeServer {
    pub(crate) catalog: Arc<AgentRuntimeCatalog>,
    composition: Arc<RuntimeComposition>,
    runner: Arc<AgentRunner>,
    services: Arc<CliServices>,
    chat: ChatLlmOptions,
    context_policy: ContextPolicy,
    default_agent: Option<String>,
    hooks: HookManager,
    run_store: Arc<dyn AgentRunStore>,
    event_store: Arc<dyn AgentRunEventStore>,
    trace_store: Arc<dyn AgentTraceStore>,
    proposal_store: Arc<dyn AgentProposalStore>,
    session_store: Arc<dyn AgentSessionStore>,
    store_path: Utf8PathBuf,
    active_runs: Arc<Mutex<HashMap<String, ActiveRun>>>,
}

fn default_agent_run_trigger() -> TriggerKind {
    TriggerKind::Manual
}

fn merge_run_metadata(metadata: Value, session_id: Option<&str>, thread_id: Option<&str>) -> Value {
    let mut metadata = if metadata.is_object() {
        metadata
    } else {
        json!({})
    };
    let object = metadata
        .as_object_mut()
        .expect("metadata was normalized to an object");
    object.insert(
        "session_id".to_owned(),
        session_id
            .map(|value| Value::String(value.to_owned()))
            .unwrap_or(Value::Null),
    );
    object.insert(
        "thread_id".to_owned(),
        thread_id
            .map(|value| Value::String(value.to_owned()))
            .unwrap_or(Value::Null),
    );
    metadata
}

fn spawn_run_event_logger(
    event_store: Arc<dyn AgentRunEventStore>,
    run_id: RunId,
    mut receiver: broadcast::Receiver<TraceEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if let Err(err) = event_store.append_run_event(&run_id, event).await {
                        warn!(
                            run_id = %run_id.0,
                            error = %err,
                            "failed to append run event log",
                        );
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        run_id = %run_id.0,
                        skipped,
                        "run event logger lagged; final trace will rewrite event log on completion",
                    );
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

async fn stop_run_event_logger(task: JoinHandle<()>) {
    task.abort();
    let _ = task.await;
}

fn serialized_value_len(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

fn apply_default_context_policy(policy: &mut ContextPolicy, configured: &ContextPolicy) {
    if *policy == ContextPolicy::default() {
        *policy = configured.clone();
    }
}

fn ensure_catalog_has_tool(catalog: &AgentRuntimeCatalog, name: &str) -> Result<()> {
    if catalog.tools.iter().any(|tool| tool.name == name) {
        return Ok(());
    }
    Err(miette!(
        "tool '{name}' is not present in the active catalog"
    ))
}

fn validated_chat_tools(
    catalog: &AgentRuntimeCatalog,
    requested: Vec<ToolSpec>,
) -> Result<Vec<ToolSpec>> {
    if requested.is_empty() {
        return Ok(catalog.tools.clone());
    }
    requested
        .into_iter()
        .map(|requested| {
            catalog
                .tools
                .iter()
                .find(|tool| tool.name == requested.name)
                .cloned()
                .ok_or_else(|| {
                    miette!(
                        "chat tool '{}' is not in the active catalog",
                        requested.name
                    )
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{chat::ChatLlmOptions, tools::ToolOverrides};

    #[test]
    fn server_context_policy_fills_only_default_chat_requests() {
        let configured = ContextPolicy {
            max_input_tokens: 1024,
            reserve_output_tokens: 128,
            preserve_recent_messages: 4,
            compact_when_over_budget: false,
        };
        let mut default_policy = ContextPolicy::default();

        apply_default_context_policy(&mut default_policy, &configured);

        assert_eq!(default_policy, configured);

        let mut client_policy = ContextPolicy {
            max_input_tokens: 2048,
            reserve_output_tokens: 256,
            preserve_recent_messages: 6,
            compact_when_over_budget: true,
        };

        apply_default_context_policy(&mut client_policy, &configured);

        assert_eq!(client_policy.max_input_tokens, 2048);
        assert_eq!(client_policy.reserve_output_tokens, 256);
        assert_eq!(client_policy.preserve_recent_messages, 6);
        assert!(client_policy.compact_when_over_budget);
    }

    #[tokio::test]
    async fn server_catalog_tools_do_not_include_agent_run_for_list_surface() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Utf8PathBuf::from_path_buf(dir.path().join("store")).expect("utf8 store");
        let registry = Utf8PathBuf::from("../../examples/agents.yaml");
        let catalog = Utf8PathBuf::from("../../fixtures/contracts/catalog.valid.json");
        let server = RuntimeServer::new(RuntimeServerOptions {
            sources: ResolvedRuntimeSources::new(registry, Some(catalog)),
            store_path: store,
            store_backend: RuntimeStoreBackend::File,
            tool_overrides: ToolOverrides::default(),
            hooks: HookManager::default(),
            context_policy: ContextPolicy::default(),
            default_agent: None,
            chat: ChatLlmOptions {
                provider: "mock".to_owned(),
                model: "mock-model".to_owned(),
                mock_response: "unused".to_owned(),
                api_base_url: None,
                api_key_env: "OPENAI_API_KEY".to_owned(),
                anthropic_version: "2023-06-01".to_owned(),
                temperature: None,
                max_output_tokens: None,
                max_tool_rounds: 4,
            },
        })
        .await
        .expect("server initializes");

        assert!(
            !server
                .catalog
                .tools
                .iter()
                .any(|tool| tool.name == "agent.run")
        );
    }

    #[test]
    fn chat_tools_are_resolved_from_catalog_authority() {
        let catalog = AgentRuntimeCatalog {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            catalog_version: agent_core::CATALOG_VERSION.to_owned(),
            generated_at: OffsetDateTime::now_utc(),
            active_domains: Vec::new(),
            agents: Vec::new(),
            tools: vec![ToolSpec {
                name: "write.data".to_owned(),
                description: "authoritative".to_owned(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                risk: agent_core::ToolRisk::High,
                metadata: json!({}),
            }],
            proposal_kinds: Vec::new(),
            prompt_blocks: Vec::new(),
        };
        let requested = ToolSpec {
            name: "write.data".to_owned(),
            description: "client override".to_owned(),
            input_schema: json!({}),
            output_schema: None,
            risk: agent_core::ToolRisk::ReadOnly,
            metadata: json!({}),
        };

        let resolved =
            validated_chat_tools(&catalog, vec![requested]).expect("catalog tool resolves");
        assert_eq!(resolved[0].risk, agent_core::ToolRisk::High);
        assert!(
            validated_chat_tools(
                &catalog,
                vec![ToolSpec {
                    name: "hidden.tool".to_owned(),
                    description: String::new(),
                    input_schema: json!({}),
                    output_schema: None,
                    risk: agent_core::ToolRisk::ReadOnly,
                    metadata: json!({}),
                }],
            )
            .is_err()
        );
    }
}
