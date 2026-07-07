use std::{collections::HashMap, sync::Arc};

use agent_chat::{ChatEventStream, ChatResumeRequest, ChatTurnRequest, ChatTurnRunner};
use agent_core::{
    AgentCancellation, AgentError, AgentEvent, AgentEventEmitter, AgentProposalStore,
    AgentRunEventStore, AgentRunRecord, AgentRunResult, AgentRunStatus, AgentRunStore,
    AgentRuntimeCatalog, AgentServices, AgentSessionStore, AgentStateAccess, AgentTraceStore,
    ApprovalLevel, ArtifactPublisher, ContextPolicy, PROTOCOL_VERSION, ProposalCreator,
    ProposalDiff, ProposalEnvelope, ProposalId, ProposalWarning, RunEventCursor, RunEventRecord,
    RunId, RunRequest, RunScope, RunWorkflow, SessionId, SessionRecord, SubagentRunner, ThreadId,
    ThreadRecord, ToolCaller, ToolError, TraceEvent, TriggerEnvelope, TriggerKind, UserContext,
    WorkflowRunRequest, WorkflowRunResult,
};
use agent_runtime::{AgentRunner, HookManager, RunControl, TraceEventBuffer};
use async_trait::async_trait;
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

#[derive(Debug, Serialize)]
pub(crate) struct AgentRunResponse {
    pub(crate) result: AgentRunResult,
    pub(crate) trace: agent_core::AgentTrace,
}

#[derive(Debug, Serialize)]
pub(crate) struct ToolCallResponse {
    tool: String,
    output: Value,
}

#[derive(Clone)]
pub(crate) struct ActiveRun {
    cancellation: CancellationToken,
    events: broadcast::Sender<TraceEvent>,
    event_buffer: Arc<TraceEventBuffer>,
}

pub(crate) struct ActiveRunEvents {
    pub(crate) receiver: broadcast::Receiver<TraceEvent>,
    pub(crate) replayed_events: Vec<TraceEvent>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CancelRunResponse {
    pub(crate) cancellation_requested: bool,
    pub(crate) message: String,
    pub(crate) run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<AgentRunStatus>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpProposalCreateParams {
    pub(crate) run_id: String,
    pub(crate) agent_id: String,
    pub(crate) kind: String,
    pub(crate) summary: String,
    #[serde(default)]
    pub(crate) payload: Value,
    #[serde(default)]
    pub(crate) diffs: Vec<ProposalDiff>,
    #[serde(default)]
    pub(crate) warnings: Vec<ProposalWarning>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpProposalListParams {
    #[serde(default)]
    pub(crate) run_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpRunListParams {
    #[serde(default)]
    pub(crate) agent_id: Option<String>,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpProposalDecisionParams {
    pub(crate) decision: String,
    #[serde(default)]
    pub(crate) approval_level: Option<ApprovalLevel>,
    #[serde(default)]
    pub(crate) decided_by: Option<String>,
    #[serde(default)]
    pub(crate) comment: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AgentRunParams {
    pub(crate) agent_id: String,
    #[serde(flatten)]
    pub(crate) run: HttpAgentRunParams,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpAgentRunParams {
    #[serde(default)]
    pub(crate) run_id: Option<String>,
    #[serde(default)]
    pub(crate) input: Value,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) thread_id: Option<String>,
    #[serde(default = "default_agent_run_trigger")]
    pub(crate) trigger: TriggerKind,
    #[serde(default)]
    pub(crate) trigger_envelope: Option<TriggerEnvelope>,
    #[serde(default)]
    pub(crate) workflow: Option<RunWorkflow>,
    #[serde(default)]
    pub(crate) user: Option<UserContext>,
    #[serde(default)]
    pub(crate) scope: Option<RunScope>,
    #[serde(default)]
    pub(crate) metadata: Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpToolCallParams {
    #[serde(default)]
    pub(crate) input: Value,
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

impl RuntimeServer {
    pub(crate) async fn new(
        sources: ResolvedRuntimeSources,
        store_path: Utf8PathBuf,
        store_backend: RuntimeStoreBackend,
        mut tool_overrides: ToolOverrides,
        hooks: HookManager,
        context_policy: ContextPolicy,
        default_agent: Option<String>,
        chat: ChatLlmOptions,
    ) -> Result<Self> {
        info!(
            registry = %sources.registry,
            catalog = sources.catalog.as_ref().map(|path| path.as_str()).unwrap_or("none"),
            store = %store_path,
            store_backend = ?store_backend,
            "initializing runtime server",
        );
        let composition = Arc::new(
            compose_runtime_sources(RuntimeSourceOptions {
                sources,
                tool_overrides: tool_overrides.clone(),
            })
            .await?,
        );
        tool_overrides.extend_tool_specs(composition.tool_specs.clone());
        let catalog = Arc::new(composition.catalog_view.clone());
        info!(
            agent_count = catalog.agents.len(),
            tool_count = catalog.tools.len(),
            proposal_kind_count = catalog.proposal_kinds.len(),
            active_domains = ?catalog.active_domains,
            "runtime server catalog loaded",
        );
        let stores = RuntimeStores::open(store_backend, store_path).await?;
        let store_path = stores.artifact_store_path.clone();
        let services = Arc::new(CliServices::with_stores(
            tool_overrides,
            stores.state_store.clone(),
            stores.proposal_store.clone(),
        ));
        let runner = Arc::new(
            AgentRunner::new(
                composition.registry.clone(),
                stores.run_store.clone(),
                services.clone(),
            )
            .with_lock_store(stores.lock_store.clone())
            .with_hooks(hooks.clone()),
        );
        Ok(Self {
            catalog,
            composition,
            runner,
            services,
            chat,
            context_policy,
            default_agent,
            hooks,
            run_store: stores.run_store,
            event_store: stores.event_store,
            trace_store: stores.trace_store,
            proposal_store: stores.proposal_store,
            session_store: stores.session_store,
            store_path,
            active_runs: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub(crate) async fn stream_chat_turn(
        &self,
        mut request: ChatTurnRequest,
    ) -> Result<ChatEventStream> {
        if request.agent_id.is_none() {
            request.agent_id = self.default_agent.clone();
        }
        if request.tools.is_empty() {
            request.tools = self.catalog.tools.clone();
        }
        if let Some(agent_id) = request.agent_id.as_deref() {
            request.messages = self
                .composition
                .chat_messages(agent_id, std::mem::take(&mut request.messages));
        }
        apply_default_context_policy(&mut request.context_policy, &self.context_policy);
        info!(
            turn_id = request.turn_id.as_deref().unwrap_or("none"),
            session_id = request.session_id.as_deref().unwrap_or("none"),
            thread_id = request.thread_id.as_deref().unwrap_or("none"),
            agent_id = request.agent_id.as_deref().unwrap_or("none"),
            provider = %request.provider,
            model = %request.model,
            tool_count = request.tools.len(),
            "server chat turn requested",
        );
        let mut chat = self.chat.clone();
        chat.provider = request.provider.clone();
        chat.model = request.model.clone();
        let provider = provider_from_options(&chat)?;
        let thread_id =
            ensure_thread(self.session_store.as_ref(), request.thread_id.as_deref()).await?;
        let services = self.chat_services(request.agent_id.clone());
        let stream = ChatTurnRunner::new(provider, services).stream(request);
        Ok(self.persist_chat_steps(stream, thread_id))
    }

    pub(crate) async fn stream_chat_resume(
        &self,
        mut request: ChatResumeRequest,
    ) -> Result<ChatEventStream> {
        if request.state.agent_id.is_none() {
            request.state.agent_id = self.default_agent.clone();
        }
        if request.state.tools.is_empty() {
            request.state.tools = self.catalog.tools.clone();
        }
        if let Some(agent_id) = request.state.agent_id.as_deref() {
            request.state.messages = self
                .composition
                .chat_messages(agent_id, std::mem::take(&mut request.state.messages));
        }
        apply_default_context_policy(&mut request.state.context_policy, &self.context_policy);
        info!(
            turn_id = request.state.turn_id.as_deref().unwrap_or("none"),
            session_id = request.state.session_id.as_deref().unwrap_or("none"),
            thread_id = request.state.thread_id.as_deref().unwrap_or("none"),
            agent_id = request.state.agent_id.as_deref().unwrap_or("none"),
            provider = %request.state.provider,
            model = %request.state.model,
            tool_result_count = request.tool_results.len(),
            "server chat resume requested",
        );
        let mut chat = self.chat.clone();
        chat.provider = request.state.provider.clone();
        chat.model = request.state.model.clone();
        let provider = provider_from_options(&chat)?;
        let thread_id = ensure_thread(
            self.session_store.as_ref(),
            request.state.thread_id.as_deref(),
        )
        .await?;
        let services = self.chat_services(request.state.agent_id.clone());
        let stream = ChatTurnRunner::new(provider, services).resume(request);
        Ok(self.persist_chat_steps(stream, thread_id))
    }

    fn chat_services(&self, parent_agent_id: Option<String>) -> Arc<dyn AgentServices> {
        let _ = parent_agent_id;
        Arc::new(RuntimeServerChatServices {
            inner: self.services.clone(),
        })
    }

    pub(crate) async fn run_agent(
        &self,
        agent_id: String,
        params: HttpAgentRunParams,
    ) -> Result<AgentRunResponse> {
        let started_at = std::time::Instant::now();
        let HttpAgentRunParams {
            run_id,
            input,
            session_id,
            thread_id,
            trigger,
            trigger_envelope,
            workflow,
            user,
            scope,
            metadata,
        } = params;
        let run_id = match run_id {
            Some(value) if !value.trim().is_empty() => RunId(value),
            Some(_) => return Err(miette!("run_id cannot be empty")),
            None => RunId::new_v7(),
        };
        let metadata = merge_run_metadata(metadata, session_id.as_deref(), thread_id.as_deref());
        let cancellation = CancellationToken::new();
        let (events, _) = broadcast::channel(256);
        let event_buffer = Arc::new(TraceEventBuffer::default());
        let event_log_task =
            spawn_run_event_logger(self.event_store.clone(), run_id.clone(), events.subscribe());
        {
            let mut active = self.active_runs.lock().await;
            if active.contains_key(&run_id.0) {
                event_log_task.abort();
                return Err(miette!("run '{}' is already active", run_id.0));
            }
            active.insert(
                run_id.0.clone(),
                ActiveRun {
                    cancellation: cancellation.clone(),
                    events: events.clone(),
                    event_buffer: event_buffer.clone(),
                },
            );
        }
        info!(
            run_id = %run_id.0,
            agent_id = %agent_id,
            session_id = session_id.as_deref().unwrap_or("none"),
            thread_id = thread_id.as_deref().unwrap_or("none"),
            trigger = ?trigger,
            input_bytes = serialized_value_len(&input),
            "server run_agent requested",
        );
        let mut control = RunControl::default();
        control.cancellation = cancellation;
        control.trace_events = Some(events);
        control.trace_event_buffer = Some(event_buffer);
        let outcome = self
            .runner
            .run_once_with_control(
                &agent_id,
                RunRequest {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    run_id: Some(run_id.clone()),
                    input,
                    user,
                    scope,
                    trigger,
                    trigger_envelope,
                    workflow,
                    metadata,
                },
                control,
            )
            .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.active_runs.lock().await.remove(&run_id.0);
                stop_run_event_logger(event_log_task).await;
                return Err(error).into_diagnostic();
            }
        };
        let trace_events = outcome.trace.events.clone();
        stop_run_event_logger(event_log_task).await;
        if let Err(err) = self
            .event_store
            .replace_run_events(&outcome.trace.run_id, trace_events)
            .await
        {
            warn!(
                run_id = %outcome.trace.run_id.0,
                error = %err,
                "failed to persist final run event log",
            );
        }
        let persist_result: Result<()> = async {
            record_session_step(self.session_store.as_ref(), thread_id.as_deref(), &outcome)
                .await?;
            self.trace_store
                .write_trace(outcome.trace.clone())
                .await
                .into_diagnostic()?;
            Ok(())
        }
        .await;
        self.active_runs.lock().await.remove(&run_id.0);
        persist_result?;
        info!(
            run_id = %outcome.result.run_id.0,
            agent_id = %outcome.result.agent_id,
            status = ?outcome.result.status,
            duration_ms = started_at.elapsed().as_millis(),
            "server run_agent completed",
        );
        Ok(AgentRunResponse {
            result: outcome.result,
            trace: outcome.trace,
        })
    }

    pub(crate) async fn run_workflow(
        &self,
        request: WorkflowRunRequest,
    ) -> Result<WorkflowRunResult> {
        info!(
            workflow_id = %request.workflow_id,
            node_count = request.nodes.len(),
            trigger = ?request.trigger,
            "server workflow run requested",
        );
        let result = self.runner.run_workflow(request).await.into_diagnostic()?;
        let trace_count = self
            .trace_store
            .write_workflow_traces(&result)
            .await
            .into_diagnostic()?;
        info!(
            workflow_id = %result.workflow_id,
            status = ?result.status,
            trace_count,
            "server workflow run completed",
        );
        Ok(result)
    }

    pub(crate) async fn cancel_run(&self, run_id: RunId) -> Result<CancelRunResponse> {
        let active = self.active_runs.lock().await.get(&run_id.0).cloned();

        if let Some(active) = active {
            let persisted = self
                .persist_run_cancellation_request(&run_id, "http")
                .await?;
            active.cancellation.cancel();
            return Ok(CancelRunResponse {
                cancellation_requested: true,
                message: if persisted.is_some() {
                    "cancellation requested and persisted".to_owned()
                } else {
                    "cancellation requested; run record is not persisted yet".to_owned()
                },
                run_id: run_id.0,
                status: Some(AgentRunStatus::Running),
            });
        }

        if let Some(status) = self
            .persist_run_cancellation_request(&run_id, "http")
            .await?
        {
            if status == AgentRunStatus::Running {
                return Ok(CancelRunResponse {
                    cancellation_requested: true,
                    message: "cancellation intent persisted".to_owned(),
                    run_id: run_id.0,
                    status: Some(status),
                });
            }
            return Ok(CancelRunResponse {
                cancellation_requested: false,
                message: "run is not active".to_owned(),
                run_id: run_id.0,
                status: Some(status),
            });
        }

        let run = self.get_run(run_id.clone()).await?;
        Ok(CancelRunResponse {
            cancellation_requested: false,
            message: "run is not active".to_owned(),
            run_id: run_id.0,
            status: Some(run.status),
        })
    }

    async fn persist_run_cancellation_request(
        &self,
        run_id: &RunId,
        requested_by: &str,
    ) -> Result<Option<AgentRunStatus>> {
        let Some(mut run) = self.run_store.get_run(run_id).await.into_diagnostic()? else {
            return Ok(None);
        };
        let status = run.status.clone();
        if status == AgentRunStatus::Running {
            run.request_cancellation(OffsetDateTime::now_utc(), Some(requested_by.to_owned()));
            self.run_store.update_run(run).await.into_diagnostic()?;
        }
        Ok(Some(status))
    }

    fn persist_chat_steps(
        &self,
        stream: ChatEventStream,
        thread_id: Option<ThreadId>,
    ) -> ChatEventStream {
        let Some(thread_id) = thread_id else {
            return stream;
        };
        let store = self.session_store.clone();
        Box::pin(stream.then(move |item| {
            let store = store.clone();
            let thread_id = thread_id.clone();
            async move {
                if let Ok(event) = &item
                    && let Err(err) =
                        record_chat_event_step(store.as_ref(), &thread_id, event).await
                {
                    warn!(
                        thread_id = %thread_id.0,
                        error = %err,
                        "failed to record chat session step",
                    );
                }
                item
            }
        }))
    }

    pub(crate) async fn subscribe_run_events(&self, run_id: &RunId) -> Option<ActiveRunEvents> {
        let (receiver, event_buffer) = {
            let active = self.active_runs.lock().await;
            let active = active.get(&run_id.0)?;
            (active.events.subscribe(), active.event_buffer.clone())
        };
        Some(ActiveRunEvents {
            receiver,
            replayed_events: event_buffer.events().await,
        })
    }

    pub(crate) async fn call_tool(&self, name: String, input: Value) -> Result<ToolCallResponse> {
        let started_at = std::time::Instant::now();
        info!(
            tool_name = %name,
            input_bytes = serialized_value_len(&input),
            "server tool call requested",
        );
        if let Err(err) = ensure_catalog_has_tool(&self.catalog, &name) {
            warn!(tool_name = %name, error = %err, "server rejected unknown tool");
            return Err(err);
        }
        let output = self
            .services
            .call_tool(&name, input)
            .await
            .map_err(|err| miette!(err.record.message))?;
        info!(
            tool_name = %name,
            output_bytes = serialized_value_len(&output),
            duration_ms = started_at.elapsed().as_millis(),
            "server tool call completed",
        );
        Ok(ToolCallResponse { tool: name, output })
    }

    pub(crate) async fn get_run(&self, run_id: RunId) -> Result<AgentRunRecord> {
        debug!(run_id = %run_id.0, "server get_run requested");
        self.run_store
            .get_run(&run_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("run '{}' was not found", run_id.0))
    }

    pub(crate) async fn list_runs(
        &self,
        agent_id: Option<String>,
        limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>> {
        debug!(
            agent_id = agent_id.as_deref().unwrap_or("all"),
            limit = ?limit,
            "server list_runs requested",
        );
        self.run_store
            .list_runs(agent_id.as_deref(), limit)
            .await
            .into_diagnostic()
    }

    pub(crate) async fn get_run_trace(&self, run_id: RunId) -> Result<Value> {
        debug!(run_id = %run_id.0, "server get_run_trace requested");
        let trace = self
            .trace_store
            .read_trace(&run_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("trace for run '{}' was not found", run_id.0))?;
        serde_json::to_value(trace).into_diagnostic()
    }

    pub(crate) async fn get_run_event_records_after(
        &self,
        run_id: RunId,
        after: RunEventCursor,
    ) -> Result<Option<Vec<RunEventRecord>>> {
        self.event_store
            .list_run_events_after(&run_id, after)
            .await
            .into_diagnostic()
    }

    pub(crate) async fn replay_run(&self, run_id: RunId) -> Result<ReplayExecutionReport> {
        info!(run_id = %run_id.0, "server replay_run requested");
        let trace_value = self.get_run_trace(run_id.clone()).await?;
        let source_trace: agent_core::AgentTrace = serde_json::from_value(trace_value)
            .map_err(|e| miette!("failed to parse trace for run '{}': {e}", run_id.0))?;
        replay_source_trace(
            self.runner.as_ref(),
            &self.store_path,
            source_trace,
            ReplayMode::Live,
        )
        .await
    }

    pub(crate) async fn metrics_summary(&self) -> Result<RuntimeMetricsSummary> {
        build_metrics_summary(
            &self.store_path,
            self.run_store.as_ref(),
            self.proposal_store.as_ref(),
        )
        .await
    }

    pub(crate) async fn create_proposal(
        &self,
        params: HttpProposalCreateParams,
    ) -> Result<ProposalEnvelope> {
        info!(
            run_id = %params.run_id,
            agent_id = %params.agent_id,
            proposal_kind = %params.kind,
            "server create_proposal requested",
        );
        let kind_spec = proposal_kind_spec(&self.catalog, &params.kind)?;
        let mut proposal = ProposalEnvelope::new(
            RunId(params.run_id),
            params.agent_id,
            params.kind,
            params.summary,
            params.payload,
        )
        .with_kind_policy(kind_spec);
        proposal.diffs = params.diffs;
        proposal.warnings = params.warnings;
        authorize_proposal_create_policy(&self.hooks, &self.store_path, &proposal).await?;
        self.proposal_store
            .create_proposal(proposal.clone())
            .await
            .into_diagnostic()?;
        append_proposal_created_trace_event(&self.store_path, &proposal).await?;
        Ok(proposal)
    }

    pub(crate) async fn list_proposals(
        &self,
        run_id: Option<String>,
    ) -> Result<Vec<ProposalEnvelope>> {
        let run_id = run_id.map(RunId);
        self.proposal_store
            .list_proposals(run_id.as_ref())
            .await
            .into_diagnostic()
    }

    pub(crate) async fn create_session(
        &self,
        params: HttpSessionCreateParams,
    ) -> Result<HttpSessionCreateResponse> {
        info!(title = %params.title, "server create_session requested");
        if params.title.trim().is_empty() {
            return Err(miette!("session title cannot be empty"));
        }
        let session = SessionRecord::new(params.title.clone(), params.metadata);
        let thread = ThreadRecord::root(
            session.session_id.clone(),
            Some(params.title),
            json!({"source": "http"}),
        );
        self.session_store
            .create_session(session.clone())
            .await
            .into_diagnostic()?;
        self.session_store
            .create_thread(thread.clone())
            .await
            .into_diagnostic()?;
        Ok(HttpSessionCreateResponse { session, thread })
    }

    pub(crate) async fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        self.session_store.list_sessions().await.into_diagnostic()
    }

    pub(crate) async fn show_session(&self, session_id: SessionId) -> Result<SessionShowReport> {
        let session = self
            .session_store
            .get_session(&session_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("session '{}' was not found", session_id.0))?;
        let mut threads = Vec::new();
        for thread in self
            .session_store
            .list_threads(&session.session_id)
            .await
            .into_diagnostic()?
        {
            let steps = self
                .session_store
                .list_steps(&thread.thread_id)
                .await
                .into_diagnostic()?;
            threads.push(ThreadWithSteps { thread, steps });
        }
        Ok(SessionShowReport { session, threads })
    }

    pub(crate) async fn fork_thread(
        &self,
        session_id: SessionId,
        params: HttpThreadForkParams,
    ) -> Result<ThreadForkReport> {
        info!(
            session_id = %session_id.0,
            parent_thread_id = %params.parent_thread_id,
            "server fork_thread requested",
        );
        self.session_store
            .get_session(&session_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("session '{}' was not found", session_id.0))?;
        let parent_thread_id = ThreadId(params.parent_thread_id);
        let parent = self
            .session_store
            .get_thread(&parent_thread_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("thread '{}' was not found", parent_thread_id.0))?;
        if parent.session_id != session_id {
            return Err(miette!(
                "thread '{}' does not belong to session '{}'",
                parent_thread_id.0,
                session_id.0
            ));
        }
        let thread = ThreadRecord::fork(
            session_id.clone(),
            parent_thread_id.clone(),
            params.title,
            params.metadata,
        );
        self.session_store
            .create_thread(thread.clone())
            .await
            .into_diagnostic()?;
        Ok(ThreadForkReport {
            session_id: session_id.0,
            parent_thread_id: parent_thread_id.0,
            thread,
        })
    }

    pub(crate) async fn get_proposal(&self, proposal_id: ProposalId) -> Result<ProposalEnvelope> {
        self.proposal_store
            .get_proposal(&proposal_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("proposal '{}' was not found", proposal_id.0))
    }

    pub(crate) async fn decide_proposal(
        &self,
        proposal_id: ProposalId,
        params: HttpProposalDecisionParams,
    ) -> Result<ProposalDecisionResponse> {
        info!(
            proposal_id = %proposal_id.0,
            decision = %params.decision,
            "server decide_proposal requested",
        );
        let mut proposal = self.get_proposal(proposal_id.clone()).await?;
        let decision = parse_approval_decision(&params.decision)?;
        let response = decide_proposal_with_store(
            self.proposal_store.as_ref(),
            &mut proposal,
            ProposalDecisionInput {
                decision,
                approval_level: params.approval_level,
                decided_by: params.decided_by,
                comment: params.comment,
            },
        )
        .await?;
        append_proposal_decision_trace_event(&self.store_path, &response).await?;
        Ok(response)
    }

    pub(crate) async fn apply_proposal(
        &self,
        proposal_id: ProposalId,
    ) -> Result<ProposalActionResponse> {
        self.execute_proposal_action(proposal_id, ProposalAction::Apply)
            .await
    }

    pub(crate) async fn undo_proposal(
        &self,
        proposal_id: ProposalId,
    ) -> Result<ProposalActionResponse> {
        self.execute_proposal_action(proposal_id, ProposalAction::Undo)
            .await
    }

    async fn execute_proposal_action(
        &self,
        proposal_id: ProposalId,
        action: ProposalAction,
    ) -> Result<ProposalActionResponse> {
        info!(
            proposal_id = %proposal_id.0,
            action = ?action,
            "server proposal action requested",
        );
        let mut proposal = self.get_proposal(proposal_id).await?;
        let tool = proposal_action_tool(&self.catalog, &proposal.kind)?;
        authorize_proposal_apply_policy(&self.hooks, &self.store_path, &proposal, &tool, action)
            .await?;
        let response = execute_proposal_action_with_store(
            self.proposal_store.as_ref(),
            self.services.as_ref(),
            &mut proposal,
            tool,
            action,
        )
        .await?;
        append_proposal_action_trace_event(&self.store_path, &response).await?;
        Ok(response)
    }
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

struct RuntimeServerChatServices {
    inner: Arc<CliServices>,
}

#[async_trait]
impl ToolCaller for RuntimeServerChatServices {
    async fn call_tool(&self, name: &str, input: Value) -> std::result::Result<Value, ToolError> {
        ToolCaller::call_tool(self.inner.as_ref(), name, input).await
    }

    async fn call_tool_with_cancellation(
        &self,
        name: &str,
        input: Value,
        cancellation: AgentCancellation,
    ) -> std::result::Result<Value, ToolError> {
        ToolCaller::call_tool_with_cancellation(self.inner.as_ref(), name, input, cancellation)
            .await
    }
}

#[async_trait]
impl AgentEventEmitter for RuntimeServerChatServices {
    async fn emit_event(&self, event: AgentEvent) -> std::result::Result<(), AgentError> {
        AgentEventEmitter::emit_event(self.inner.as_ref(), event).await
    }
}

#[async_trait]
impl AgentStateAccess for RuntimeServerChatServices {
    async fn load_state(&self, key: &str) -> std::result::Result<Option<Value>, AgentError> {
        AgentStateAccess::load_state(self.inner.as_ref(), key).await
    }

    async fn save_state(&self, key: &str, value: Value) -> std::result::Result<(), AgentError> {
        AgentStateAccess::save_state(self.inner.as_ref(), key, value).await
    }
}

#[async_trait]
impl ProposalCreator for RuntimeServerChatServices {
    async fn create_proposal(
        &self,
        proposal: ProposalEnvelope,
    ) -> std::result::Result<(), AgentError> {
        ProposalCreator::create_proposal(self.inner.as_ref(), proposal).await
    }
}

#[async_trait]
impl SubagentRunner for RuntimeServerChatServices {}

#[async_trait]
impl ArtifactPublisher for RuntimeServerChatServices {}

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
        let server = RuntimeServer::new(
            ResolvedRuntimeSources::new(registry, Some(catalog)),
            store,
            RuntimeStoreBackend::File,
            ToolOverrides::default(),
            HookManager::default(),
            ContextPolicy::default(),
            None,
            ChatLlmOptions {
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
        )
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
}
