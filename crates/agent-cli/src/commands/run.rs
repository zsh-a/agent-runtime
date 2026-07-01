use std::sync::Arc;

use agent_core::{PROTOCOL_VERSION, RunRequest};
use agent_runtime::AgentRunner;
use agent_store::{FileProposalStore, FileRunStore};
use camino::Utf8PathBuf;
use miette::{IntoDiagnostic, Result};
use serde_json::json;
use tracing::info;

use crate::{
    catalog::load_catalog_registry,
    config::execution_policy,
    print_json,
    registry::load_registry,
    session::{record_session_step, run_metadata},
    tools::{CliServices, ToolOverrides},
    trace_store::{read_json, write_json, write_store_trace},
};

pub(crate) struct RunCliOptions {
    pub(crate) agent_id: String,
    pub(crate) registry: Utf8PathBuf,
    pub(crate) catalog: Option<Utf8PathBuf>,
    pub(crate) tool_overrides: ToolOverrides,
    pub(crate) input: Option<Utf8PathBuf>,
    pub(crate) trace_out: Option<Utf8PathBuf>,
    pub(crate) session: Option<String>,
    pub(crate) thread: Option<String>,
    pub(crate) store: Utf8PathBuf,
    pub(crate) timeout_seconds: u64,
    pub(crate) max_retries: u32,
    pub(crate) retry_backoff_ms: u64,
}

pub(crate) async fn run_agent_once(options: RunCliOptions) -> Result<()> {
    info!(
        agent_id = %options.agent_id,
        registry = %options.registry,
        catalog = options.catalog.as_ref().map(|path| path.as_str()).unwrap_or("none"),
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
    let registry = match options.catalog {
        Some(path) => load_catalog_registry(path).await?,
        None => load_registry(options.registry).await?.into_agent_registry(),
    };
    let store_path = options.store;
    let metadata = run_metadata(options.session.as_deref(), options.thread.as_deref());
    let store = Arc::new(
        FileRunStore::new(store_path.clone())
            .await
            .into_diagnostic()?,
    );
    let proposal_store = Arc::new(
        FileProposalStore::new(store_path.clone())
            .await
            .into_diagnostic()?,
    );
    let services = Arc::new(CliServices::with_proposal_store(
        options.tool_overrides,
        proposal_store,
    ));
    let runner = AgentRunner::new(registry, store, services).with_policy(execution_policy(
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
                trigger: agent_core::TriggerKind::Manual,
                metadata,
            },
        )
        .await
        .into_diagnostic()?;
    record_session_step(&store_path, options.thread.as_deref(), &outcome).await?;
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
    pub(crate) registry: Utf8PathBuf,
    pub(crate) store: Utf8PathBuf,
}

pub(crate) async fn tick_agents(options: TickCliOptions) -> Result<()> {
    info!(
        registry = %options.registry,
        store = %options.store,
        "starting scheduler tick",
    );
    let registry = load_registry(options.registry).await?;
    let store_path = options.store;
    let store = Arc::new(
        FileRunStore::new(store_path.clone())
            .await
            .into_diagnostic()?,
    );
    let proposal_store = Arc::new(
        FileProposalStore::new(store_path.clone())
            .await
            .into_diagnostic()?,
    );
    let services = Arc::new(CliServices::with_proposal_store(
        ToolOverrides::default(),
        proposal_store,
    ));
    let runner = AgentRunner::new(registry.into_agent_registry(), store, services);
    let outcomes = runner
        .tick(RunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: None,
            input: json!({}),
            user: None,
            trigger: agent_core::TriggerKind::Scheduled,
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
