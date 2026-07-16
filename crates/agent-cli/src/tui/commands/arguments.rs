use super::*;

pub(super) fn split_once(input: &str) -> (&str, &str) {
    input
        .trim()
        .split_once(char::is_whitespace)
        .map(|(head, tail)| (head, tail.trim()))
        .unwrap_or((input.trim(), ""))
}

pub(super) fn split_name_and_json<'a>(input: &'a str, label: &str) -> Result<(String, &'a str)> {
    let input = input.trim();
    if input.is_empty() {
        return Err(miette!("{label} is required"));
    }
    let (name, rest) = split_once(input);
    if name.trim().is_empty() {
        return Err(miette!("{label} is required"));
    }
    Ok((name.to_owned(), rest))
}

pub(super) fn parse_json_or_default(input: &str, label: &str) -> Result<Value> {
    if input.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(input).map_err(|e| miette!("failed to parse {label} as JSON: {e}"))
}

pub(super) fn parse_run_input(input: &str) -> Result<Value> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_str(input) {
        Ok(value) => Ok(value),
        Err(_) => Ok(json!({"message": input})),
    }
}

pub(super) fn push_agent_output(state: &mut TuiState, output: &Value) {
    if let Some(message) = output.get("message").and_then(Value::as_str) {
        state.push_assistant_message(message.to_owned());
    } else if let Some(content) = output.get("content").and_then(Value::as_str) {
        state.push_assistant_message(content.to_owned());
    } else {
        state.push_assistant_message(pretty_json(output));
    }
}

pub(super) async fn load_proposals(
    store_backend: crate::config::RuntimeStoreBackend,
    store_path: &Utf8PathBuf,
    run_id: Option<&RunId>,
) -> Result<Vec<ProposalEnvelope>> {
    let stores = RuntimeStores::open(store_backend, store_path.clone()).await?;
    stores
        .proposal_store
        .list_proposals(run_id)
        .await
        .into_diagnostic()
}

pub(super) async fn load_proposal(
    store_backend: crate::config::RuntimeStoreBackend,
    store_path: &Utf8PathBuf,
    proposal_id: &ProposalId,
) -> Result<ProposalEnvelope> {
    let stores = RuntimeStores::open(store_backend, store_path.clone()).await?;
    stores
        .proposal_store
        .get_proposal(proposal_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("proposal '{}' was not found", proposal_id.0))
}

pub(super) async fn load_runs(
    store_backend: crate::config::RuntimeStoreBackend,
    store_path: &Utf8PathBuf,
    limit: usize,
) -> Result<Vec<agent_core::AgentRunRecord>> {
    let stores = RuntimeStores::open(store_backend, store_path.clone()).await?;
    stores
        .run_store
        .list_runs(None, Some(limit))
        .await
        .into_diagnostic()
}

pub(super) fn optional_run_id(input: &str) -> Result<Option<RunId>> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    if input.split_whitespace().count() > 1 {
        return Err(miette!("expected at most one run_id"));
    }
    Ok(Some(RunId(input.to_owned())))
}

pub(super) fn run_list_limit(input: &str) -> Result<usize> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(8);
    }
    if input.split_whitespace().count() > 1 {
        return Err(miette!("expected /runs [limit]"));
    }
    let limit = input
        .parse::<usize>()
        .map_err(|e| miette!("run limit must be a positive integer: {e}"))?;
    Ok(limit.clamp(1, 50))
}

pub(super) fn run_id_arg(input: &str) -> Result<RunId> {
    let input = input.trim();
    if input.is_empty() {
        return Err(miette!("run id is required"));
    }
    let (run_id, rest) = split_once(input);
    if !rest.is_empty() {
        return Err(miette!("unexpected extra input after run id"));
    }
    Ok(RunId(run_id.to_owned()))
}

pub(super) fn run_id_arg_or_default(state: &TuiState, input: &str) -> Result<RunId> {
    let input = input.trim();
    if input.is_empty() {
        return default_run_id(state);
    }
    run_id_arg(input)
}

pub(super) fn default_run_id(state: &TuiState) -> Result<RunId> {
    if let Some(trace) = &state.trace {
        return Ok(trace.run_id.clone());
    }
    if let Some(run) = &state.latest_run {
        return Ok(RunId(run.run_id.clone()));
    }
    if let Some(run) = state.recent_runs.first() {
        return Ok(run.run_id.clone());
    }
    Err(miette!("run id is required; use /runs to list recent runs"))
}

pub(super) fn run_events_args(state: &TuiState, input: &str) -> Result<(RunId, usize)> {
    let input = input.trim();
    if input.is_empty() {
        return Ok((default_run_id(state)?, 12));
    }
    let mut parts = input.split_whitespace();
    let first = parts
        .next()
        .ok_or_else(|| miette!("run id is required"))?
        .to_owned();
    let Some(second) = parts.next() else {
        if let Ok(limit) = first.parse::<usize>() {
            return Ok((default_run_id(state)?, limit.clamp(1, 50)));
        }
        return Ok((RunId(first), 12));
    };
    let limit = match parts.next() {
        None => second
            .parse::<usize>()
            .map_err(|e| miette!("event limit must be a positive integer: {e}"))?
            .clamp(1, 50),
        Some(_) => return Err(miette!("expected /events [run_id] [limit]")),
    };
    Ok((RunId(first), limit))
}

pub(super) fn proposal_id_arg(input: &str) -> Result<ProposalId> {
    let input = input.trim();
    if input.is_empty() {
        return Err(miette!("proposal id is required"));
    }
    let (proposal_id, rest) = split_once(input);
    if !rest.is_empty() {
        return Err(miette!("unexpected extra input after proposal id"));
    }
    Ok(ProposalId(proposal_id.to_owned()))
}

pub(super) fn proposal_id_arg_or_default(state: &TuiState, input: &str) -> Result<ProposalId> {
    let input = input.trim();
    if !input.is_empty() {
        return proposal_id_arg(input);
    }
    let Some(proposals) = &state.latest_proposals else {
        return Err(miette!(
            "proposal id is required; use /proposals to list proposals"
        ));
    };
    match proposals.proposals.as_slice() {
        [proposal] => Ok(ProposalId(proposal.proposal_id.clone())),
        [] => Err(miette!(
            "proposal id is required; no proposals are currently shown"
        )),
        proposals => Err(miette!(
            "proposal id is required; {} proposals are currently shown",
            proposals.len()
        )),
    }
}

pub(super) fn proposal_decision_args(input: &str) -> Result<(ProposalId, Option<String>)> {
    let input = input.trim();
    if input.is_empty() {
        return Err(miette!("proposal id is required"));
    }
    let (proposal_id, comment) = split_once(input);
    let comment = (!comment.is_empty()).then(|| comment.to_owned());
    Ok((ProposalId(proposal_id.to_owned()), comment))
}
