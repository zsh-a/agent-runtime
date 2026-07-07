use std::sync::Arc;

use agent_core::{PROTOCOL_VERSION, RunRequest, RunScope};
use agent_runtime::{AgentRunner, HookManager};
use camino::Utf8PathBuf;
use miette::{IntoDiagnostic, Result};
use serde_json::json;
use tracing::info;

use crate::{
    config::execution_policy,
    print_json,
    runtime_config::{ResolvedRuntimeSources, RuntimeSourceOptions, compose_runtime_sources},
    runtime_stores::RuntimeStores,
    session::{record_session_step, run_metadata},
    tools::{CliServices, ToolOverrides},
    trace_store::{read_json, write_json, write_store_trace},
};

pub(crate) struct RunCliOptions {
    pub(crate) agent_id: String,
    pub(crate) sources: ResolvedRuntimeSources,
    pub(crate) tool_overrides: ToolOverrides,
    pub(crate) input: Option<Utf8PathBuf>,
    pub(crate) trace_out: Option<Utf8PathBuf>,
    pub(crate) session: Option<String>,
    pub(crate) thread: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) store: Utf8PathBuf,
    pub(crate) store_backend: crate::config::RuntimeStoreBackend,
    pub(crate) timeout_seconds: u64,
    pub(crate) max_retries: u32,
    pub(crate) retry_backoff_ms: u64,
    pub(crate) hooks: HookManager,
}

pub(crate) async fn run_agent_once(options: RunCliOptions) -> Result<()> {
    info!(
        agent_id = %options.agent_id,
        registry = %options.sources.registry,
        catalog = options.sources.catalog.as_ref().map(|path| path.as_str()).unwrap_or("none"),
        store = %options.store,
        input = options.input.as_ref().map(|path| path.as_str()).unwrap_or("none"),
        trace_out = options.trace_out.as_ref().map(|path| path.as_str()).unwrap_or("none"),
        timeout_seconds = options.timeout_seconds,
        max_retries = options.max_retries,
        retry_backoff_ms = options.retry_backoff_ms,
        "running agent once",
    );
    let input = match options.input {
        Some(path) => read_json(path).await?,
        None => json!({}),
    };
    let mut tool_overrides = options.tool_overrides;
    let composition = compose_runtime_sources(RuntimeSourceOptions {
        sources: options.sources,
        tool_overrides: tool_overrides.clone(),
    })
    .await?;
    tool_overrides.extend_tool_specs(composition.tool_specs.clone());
    let stores = RuntimeStores::open(options.store_backend, options.store).await?;
    let store_path = stores.artifact_store_path.clone();
    let metadata = run_metadata(options.session.as_deref(), options.thread.as_deref());
    let scope = parse_run_scope(options.scope.as_deref())?;
    let services = Arc::new(CliServices::with_stores(
        tool_overrides,
        stores.state_store.clone(),
        stores.proposal_store.clone(),
    ));
    let runner = AgentRunner::new(composition.registry, stores.run_store.clone(), services)
        .with_lock_store(stores.lock_store.clone())
        .with_hooks(options.hooks)
        .with_policy(execution_policy(
            options.timeout_seconds,
            options.max_retries,
            options.retry_backoff_ms,
        ));
    let outcome = runner
        .run_once(
            &options.agent_id,
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: None,
                input,
                user: None,
                scope,
                trigger: agent_core::TriggerKind::Manual,
                trigger_envelope: None,
                workflow: None,
                metadata,
            },
        )
        .await
        .into_diagnostic()?;
    record_session_step(
        stores.session_store.as_ref(),
        options.thread.as_deref(),
        &outcome,
    )
    .await?;
    write_store_trace(&store_path, &outcome.trace).await?;
    if let Some(path) = options.trace_out {
        write_json(path, &outcome.trace).await?;
    }
    info!(
        run_id = %outcome.result.run_id.0,
        agent_id = %outcome.result.agent_id,
        status = ?outcome.result.status,
        store = %store_path,
        "agent run artifacts written",
    );
    print_json(&outcome.result)
}

pub(crate) struct TickCliOptions {
    pub(crate) sources: ResolvedRuntimeSources,
    pub(crate) store: Utf8PathBuf,
    pub(crate) store_backend: crate::config::RuntimeStoreBackend,
    pub(crate) hooks: HookManager,
}

pub(crate) async fn tick_agents(options: TickCliOptions) -> Result<()> {
    info!(
        registry = %options.sources.registry,
        catalog = options.sources.catalog.as_ref().map(|path| path.as_str()).unwrap_or("none"),
        store = %options.store,
        "starting scheduler tick",
    );
    let mut tool_overrides = ToolOverrides::default();
    let composition = compose_runtime_sources(RuntimeSourceOptions {
        sources: options.sources,
        tool_overrides: tool_overrides.clone(),
    })
    .await?;
    tool_overrides.extend_tool_specs(composition.tool_specs.clone());
    let stores = RuntimeStores::open(options.store_backend, options.store).await?;
    let store_path = stores.artifact_store_path.clone();
    let services = Arc::new(CliServices::with_stores(
        tool_overrides,
        stores.state_store.clone(),
        stores.proposal_store.clone(),
    ));
    let runner = AgentRunner::new(composition.registry, stores.run_store.clone(), services)
        .with_lock_store(stores.lock_store.clone())
        .with_hooks(options.hooks);
    let outcomes = runner
        .tick(RunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: None,
            input: json!({}),
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Scheduled,
            trigger_envelope: None,
            workflow: None,
            metadata: json!({}),
        })
        .await
        .into_diagnostic()?;
    for outcome in &outcomes {
        write_store_trace(&store_path, &outcome.trace).await?;
    }
    let results = outcomes
        .into_iter()
        .map(|outcome| outcome.result)
        .collect::<Vec<_>>();
    info!(
        run_count = results.len(),
        "scheduler tick artifacts written"
    );
    print_json(&results)
}

fn parse_run_scope(value: Option<&str>) -> Result<Option<RunScope>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.eq_ignore_ascii_case("global") {
        return Ok(Some(RunScope::Global));
    }
    if let Some(user_id) = value.strip_prefix("user:") {
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return Err(miette::miette!("--scope user:ID requires a non-empty ID"));
        }
        return Ok(Some(RunScope::User(user_id.to_owned())));
    }
    if let Some(tenant_id) = value.strip_prefix("tenant:") {
        let tenant_id = tenant_id.trim();
        if tenant_id.is_empty() {
            return Err(miette::miette!("--scope tenant:ID requires a non-empty ID"));
        }
        return Ok(Some(RunScope::Tenant(tenant_id.to_owned())));
    }
    Err(miette::miette!(
        "--scope must be one of: global, user:ID, tenant:ID"
    ))
}
