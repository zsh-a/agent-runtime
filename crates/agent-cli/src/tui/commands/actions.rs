use super::*;

pub(super) async fn show_agents_command(state: &mut TuiState) -> Result<()> {
    let runtime = TuiRuntime::load(&state.options).await?;
    let active_agent_id = runtime.resolve_agent_id(state.selected_agent_id.as_deref())?;
    state.push_user_message("/agents");
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::System,
        "agents listed",
        format!("{} available", runtime.agent_specs().len()),
    ));
    state.push_system_message(format_agent_list(runtime.agent_specs(), &active_agent_id));
    state.detail_kind = TuiDetailKind::Overview;
    state.focus_panel(TuiFocusPanel::Context);
    Ok(())
}

pub(super) async fn use_agent_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let agent_id = rest.trim();
    if agent_id.is_empty() {
        return Err(miette!("agent id is required"));
    }
    let runtime = TuiRuntime::load(&state.options).await?;
    let agent_id = runtime.resolve_agent_id(Some(agent_id))?;
    state.push_user_message(format!("/use {agent_id}"));
    state.set_selected_agent(agent_id.clone());
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::System,
        "agent selected",
        agent_id.clone(),
    ));
    state.push_system_message(format!(
        "Using agent '{agent_id}' for natural-language chat."
    ));
    Ok(())
}

pub(super) async fn show_tools_command(state: &mut TuiState) -> Result<()> {
    let inventory = load_tui_tool_inventory(&state.options).await?;
    state.tool_inventory = Some(inventory.clone());
    state.push_user_message("/tools");
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Tool,
        "tools listed",
        format!(
            "{} available, {} high-risk, {} blocked",
            inventory.total_count(),
            inventory.high_risk_count(),
            inventory.blocked_count()
        ),
    ));
    state.push_system_message(format_tool_inventory(&inventory));
    state.detail_kind = TuiDetailKind::Overview;
    state.focus_panel(TuiFocusPanel::Context);
    Ok(())
}

pub(super) async fn run_agent_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let (agent_id, json_input) = split_name_and_json(rest, "agent id")?;
    let input = parse_run_input(json_input)?;
    state.push_user_message(format!("/run {agent_id} {}", compact_json(&input)));
    run_agent_with_input(state, &agent_id, input, "slash_command").await
}

pub(super) async fn list_runs_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let limit = run_list_limit(rest)?;
    let runs = load_runs(
        state.options.store_backend,
        &state.options.store_path,
        limit,
    )
    .await?;
    state.set_recent_runs(runs.clone());
    state.push_user_message(if rest.trim().is_empty() {
        "/runs".to_owned()
    } else {
        format!("/runs {limit}")
    });
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Run,
        "runs listed",
        format!("{} shown", runs.len()),
    ));
    state.push_system_message(format_run_list(&runs));
    state.focus_panel(TuiFocusPanel::Activity);
    Ok(())
}

pub(super) async fn cancel_run_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let run_id = run_id_arg(rest)?;
    let runtime = TuiRuntime::load(&state.options).await?;
    let result = runtime.cancel_run(run_id).await?;
    state.push_user_message(format!("/cancel {}", result.run_id.0));
    state.refresh_runs().await?;
    let title = if result.cancellation_requested {
        "cancellation requested"
    } else {
        "run not cancelled"
    };
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Cancellation,
        title,
        format!("{} {}", result.run_id.0, status_label(&result.status)),
    ));
    state.push_system_message(format_cancel_result(&result));
    Ok(())
}

pub(super) async fn run_workflow_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let path = rest.trim();
    if path.is_empty() {
        return Err(miette!("workflow path is required"));
    }
    let path = Utf8PathBuf::from(path);
    let request = read_workflow_request(path.clone()).await?;
    let workflow_id = request.workflow_id.clone();
    state.push_user_message(format!("/workflow {path}"));

    let runtime = TuiRuntime::load(&state.options).await?;
    let result = runtime.run_workflow(request).await?;
    if let Some((node_id, trace)) = result.nodes.iter().find_map(|node| {
        node.trace
            .as_ref()
            .map(|trace| (node.node_id.as_str(), trace))
    }) {
        state.set_trace(
            format!("workflow {workflow_id} node {node_id}"),
            trace.clone(),
        );
    }
    state.refresh_runs().await?;
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Run,
        format!("workflow {}", result.workflow_id),
        format!(
            "{} nodes {}",
            result.nodes.len(),
            status_label(&result.status)
        ),
    ));
    state.set_latest_workflow(workflow_summary_from_result(&result));
    state.push_system_message(format_workflow_result(&result));
    Ok(())
}

pub(super) async fn load_run_events_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let (run_id, limit) = run_events_args(state, rest)?;
    let trace = load_store_trace(
        state.options.store_backend,
        &state.options.store_path,
        &run_id,
    )
    .await?;
    let summary = trace_event_summary(&trace, limit);
    state.set_trace(format!("store trace {}", run_id.0), trace);
    state.set_latest_events(summary.clone());
    state.push_user_message(format!("/events {} {}", run_id.0, limit));
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::System,
        "events loaded",
        format!(
            "{} showing {}/{}",
            run_id.0, summary.shown_count, summary.event_count
        ),
    ));
    for event in &summary.events {
        state.push_activity(TuiActivityItem::with_detail(
            activity_kind_from_event(&event.kind),
            event.kind.clone(),
            event.detail.clone().unwrap_or_default(),
        ));
    }
    state.push_system_message(format_trace_event_summary(&summary));
    Ok(())
}

pub(super) async fn list_proposals_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let run_id = optional_run_id(rest)?;
    let proposals = load_proposals(
        state.options.store_backend,
        &state.options.store_path,
        run_id.as_ref(),
    )
    .await?;
    let summary = proposal_list_summary(&proposals);
    state.set_latest_proposals(summary);
    state.push_user_message(match &run_id {
        Some(run_id) => format!("/proposals {}", run_id.0),
        None => "/proposals".to_owned(),
    });
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Approval,
        "proposals listed",
        format!("{} total", proposals.len()),
    ));
    state.push_system_message(format_proposal_list(&proposals));
    Ok(())
}

pub(super) async fn inspect_proposal_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let proposal_id = proposal_id_arg_or_default(state, rest)?;
    let proposal = load_proposal(
        state.options.store_backend,
        &state.options.store_path,
        &proposal_id,
    )
    .await?;
    state.set_latest_proposals(proposal_list_summary(std::slice::from_ref(&proposal)));
    state.push_user_message(format!("/proposal {}", proposal.proposal_id.0));
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Approval,
        format!("proposal {}", proposal.proposal_id.0),
        proposal_status_label(&proposal.status),
    ));
    state.push_system_message(pretty_json(&proposal));
    Ok(())
}

pub(super) async fn decide_proposal_command(
    state: &mut TuiState,
    rest: &str,
    decision: ApprovalDecisionKind,
) -> Result<()> {
    let (proposal_id, comment) = proposal_decision_args(rest)?;
    let stores = RuntimeStores::open(
        state.options.store_backend,
        state.options.store_path.clone(),
    )
    .await?;
    let mut proposal = stores
        .proposal_store
        .get_proposal(&proposal_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("proposal '{}' was not found", proposal_id.0))?;
    let response = decide_proposal_with_store(
        stores.proposal_store.as_ref(),
        &mut proposal,
        ProposalDecisionInput {
            decision,
            approval_level: Some(ApprovalLevel::SingleUser),
            decided_by: Some("agent_tui".to_owned()),
            comment,
        },
    )
    .await?;
    append_proposal_decision_trace_event(stores.trace_store.as_ref(), &response).await?;
    state.set_latest_proposals(proposal_list_summary(std::slice::from_ref(
        &response.proposal,
    )));
    let decision_label = approval_decision_label(&response.decision.decision);
    state.push_user_message(format!("/{decision_label}-proposal {}", proposal_id.0));
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Approval,
        format!(
            "proposal {}",
            approval_decision_past_tense(&response.decision.decision)
        ),
        response.proposal.proposal_id.0.clone(),
    ));
    state.push_system_message(format!(
        "Proposal {} {}. Status: {}",
        response.proposal.proposal_id.0,
        approval_decision_past_tense(&response.decision.decision),
        proposal_status_label(&response.proposal.status)
    ));
    Ok(())
}

pub(super) async fn run_agent_with_input(
    state: &mut TuiState,
    agent_id: &str,
    input: Value,
    input_mode: &str,
) -> Result<()> {
    let runtime = TuiRuntime::load(&state.options).await?;
    let outcome = runtime.run_agent_once(agent_id, input, input_mode).await?;
    state.set_trace(
        format!("latest run {}", outcome.result.run_id.0),
        outcome.trace,
    );
    state.refresh_runs().await?;
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Run,
        format!("run {}", outcome.result.run_id.0),
        format!("{} {:?}", outcome.result.agent_id, outcome.result.status),
    ));
    if let Some(summary) = outcome.result.summary {
        state.push_activity(TuiActivityItem::with_detail(
            TuiActivityKind::Run,
            "summary",
            summary,
        ));
    }
    push_agent_output(state, &outcome.result.output);
    Ok(())
}

pub(super) async fn tool_call_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let (name, json_input) = split_name_and_json(rest, "tool name")?;
    let input = parse_json_or_default(json_input, "tool input")?;
    state.push_user_message(format!("/tool {name} {}", compact_json(&input)));
    call_tool_or_request_approval(state, name, input).await
}

pub(super) async fn load_trace_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let path = rest.trim();
    if path.is_empty() {
        return Err(miette!("trace path is required"));
    }
    let path = Utf8PathBuf::from(path);
    let trace = read_trace(path.clone()).await?;
    state.set_trace(path.to_string(), trace);
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::System,
        "loaded trace",
        path.to_string(),
    ));
    Ok(())
}

pub(super) async fn load_store_trace(
    store_backend: crate::config::RuntimeStoreBackend,
    store_path: &Utf8PathBuf,
    run_id: &RunId,
) -> Result<AgentTrace> {
    RuntimeStores::open(store_backend, store_path.clone())
        .await?
        .trace_store
        .read_trace(run_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("trace for run '{}' was not found", run_id.0))
}

pub(super) async fn inspect_run_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let run_id = run_id_arg_or_default(state, rest)?;
    let stores = RuntimeStores::open(
        state.options.store_backend,
        state.options.store_path.clone(),
    )
    .await?;
    let record = stores
        .run_store
        .get_run(&run_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("run '{}' was not found", run_id.0))?;
    let summary = run_summary(&record);
    state.set_latest_run(summary.clone());
    state.push_user_message(format!("/inspect {}", run_id.0));
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Run,
        format!("run {}", record.run_id.0),
        format!("{} {}", record.agent_id, status_label(&record.status)),
    ));
    state.push_system_message(format_run_summary(&summary));
    Ok(())
}
