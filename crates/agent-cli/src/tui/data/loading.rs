use super::*;

pub(in crate::tui) fn select_initial_agent(
    default_agent: Option<&str>,
    agents: &[TuiAgentSummary],
) -> Result<Option<String>> {
    if let Some(default_agent) = default_agent.filter(|agent_id| !agent_id.trim().is_empty()) {
        if agents.iter().any(|agent| agent.id == default_agent) {
            return Ok(Some(default_agent.to_owned()));
        }
        return Err(miette!(
            "configured default agent '{default_agent}' was not found"
        ));
    }
    Ok(agents.first().map(|agent| agent.id.clone()))
}

pub(in crate::tui) async fn load_catalog_summary(
    path: Option<&Utf8Path>,
) -> Result<Option<CatalogSummary>> {
    match path {
        Some(path) => Ok(Some(CatalogSummary::from_catalog(
            &read_catalog(path.to_owned()).await?,
        ))),
        None => Ok(None),
    }
}

pub(in crate::tui) async fn load_agent_summaries(
    options: &TuiOptions,
) -> Result<Vec<TuiAgentSummary>> {
    let composition = compose_runtime_sources(RuntimeSourceOptions {
        sources: options.runtime_sources.clone(),
        tool_overrides: options.tool_overrides.clone(),
    })
    .await?;
    Ok(agent_summaries(composition.agent_specs.iter()))
}

pub(in crate::tui) fn agent_summaries<'a>(
    agents: impl IntoIterator<Item = &'a AgentSpec>,
) -> Vec<TuiAgentSummary> {
    agents
        .into_iter()
        .map(|agent| TuiAgentSummary {
            id: agent.id.clone(),
            name: agent.name.clone(),
        })
        .collect()
}

pub(in crate::tui) async fn load_trace(
    path: Option<&Utf8PathBuf>,
) -> Result<Option<agent_core::AgentTrace>> {
    match path {
        Some(path) => Ok(Some(read_trace(path.clone()).await?)),
        None => Ok(None),
    }
}

pub(in crate::tui) async fn read_recent_runs(
    store_backend: RuntimeStoreBackend,
    store_path: &Utf8Path,
) -> Result<Vec<AgentRunRecord>> {
    let stores = RuntimeStores::open(store_backend, store_path.to_owned()).await?;
    stores
        .run_store
        .list_runs(None, Some(8))
        .await
        .into_diagnostic()
}

pub(in crate::tui) async fn read_trace(path: Utf8PathBuf) -> Result<agent_core::AgentTrace> {
    let value = read_json(path.clone()).await?;
    serde_json::from_value(value).map_err(|e| miette!("failed to parse trace at {path}: {e}"))
}

pub(in crate::tui) async fn read_json(path: Utf8PathBuf) -> Result<Value> {
    let bytes = fs_err::tokio::read(&path)
        .await
        .map_err(|e| miette!("failed to read JSON at {path}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| miette!("failed to parse JSON at {path}: {e}"))
}

pub(in crate::tui) fn status_line(
    selected_agent_id: Option<&str>,
    catalog_summary: &Option<CatalogSummary>,
    trace: &Option<agent_core::AgentTrace>,
    run_count: usize,
) -> String {
    format!(
        "agent {} | catalog {} | trace {} | runs {}",
        selected_agent_id.unwrap_or("auto"),
        catalog_summary
            .as_ref()
            .map(|summary| summary.agent_count.to_string())
            .unwrap_or_else(|| "-".to_owned()),
        trace
            .as_ref()
            .map(|trace| trace.run_id.0.clone())
            .unwrap_or_else(|| "-".to_owned()),
        run_count
    )
}
