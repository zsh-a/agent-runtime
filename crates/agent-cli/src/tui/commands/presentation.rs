use super::*;

pub(super) fn proposal_list_summary(proposals: &[ProposalEnvelope]) -> TuiProposalListSummary {
    TuiProposalListSummary {
        total_count: proposals.len(),
        pending_count: proposals
            .iter()
            .filter(|proposal| proposal_pending(&proposal.status))
            .count(),
        approved_count: proposals
            .iter()
            .filter(|proposal| proposal.status == ProposalStatus::Approved)
            .count(),
        denied_count: proposals
            .iter()
            .filter(|proposal| proposal.status == ProposalStatus::Denied)
            .count(),
        proposals: proposals.iter().map(proposal_summary).collect(),
    }
}

pub(super) fn proposal_summary(proposal: &ProposalEnvelope) -> TuiProposalSummary {
    TuiProposalSummary {
        proposal_id: proposal.proposal_id.0.clone(),
        run_id: proposal.run_id.0.clone(),
        agent_id: proposal.agent_id.clone(),
        kind: proposal.kind.clone(),
        summary: proposal.summary.clone(),
        status: proposal_status_label(&proposal.status).to_owned(),
        risk: format!("{:?}", proposal.risk),
        diff_count: proposal.diffs.len(),
        warning_count: proposal.warnings.len(),
    }
}

pub(super) fn trace_event_summary(trace: &AgentTrace, limit: usize) -> TuiTraceEventSummary {
    let event_count = trace.events.len();
    let start = event_count.saturating_sub(limit);
    let events = trace.events[start..]
        .iter()
        .map(trace_event_item)
        .collect::<Vec<_>>();
    TuiTraceEventSummary {
        run_id: trace.run_id.0.clone(),
        agent_id: trace.agent_id.clone(),
        event_count,
        shown_count: events.len(),
        events,
    }
}

pub(super) fn trace_event_item(event: &TraceEvent) -> TuiTraceEventItem {
    TuiTraceEventItem {
        kind: event.kind.clone(),
        detail: trace_event_detail(event),
    }
}

pub(super) fn trace_event_detail(event: &TraceEvent) -> Option<String> {
    let object = event.payload.as_object()?;
    let mut parts = Vec::new();
    for key in [
        "agent_id",
        "tool_name",
        "status",
        "reason",
        "proposal_id",
        "decision",
        "action",
        "duration_ms",
    ] {
        if let Some(value) = object.get(key).filter(|value| !value.is_null()) {
            parts.push(format!("{key}={}", compact_event_value(value)));
        }
    }
    if let Some(error) = object.get("error").filter(|value| !value.is_null()) {
        parts.push(format!("error={}", compact_event_value(error)));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

pub(super) fn compact_event_value(value: &Value) -> String {
    let raw = value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| compact_json(value));
    compact_description(&raw)
}

pub(super) fn format_trace_event_summary(summary: &TuiTraceEventSummary) -> String {
    let mut lines = vec![
        format!(
            "Events for run {}: showing {}/{}",
            summary.run_id, summary.shown_count, summary.event_count
        ),
        format!("Agent: {}", summary.agent_id),
    ];
    if summary.events.is_empty() {
        lines.push("No trace events found.".to_owned());
    } else {
        lines.extend(summary.events.iter().map(|event| {
            let detail = event
                .detail
                .as_deref()
                .filter(|detail| !detail.is_empty())
                .map(|detail| format!(" {detail}"))
                .unwrap_or_default();
            format!("- {}{}", event.kind, detail)
        }));
    }
    lines.join("\n")
}

pub(super) fn activity_kind_from_event(kind: &str) -> TuiActivityKind {
    let kind = kind.to_ascii_lowercase();
    if kind.contains("tool") {
        TuiActivityKind::Tool
    } else if kind.contains("proposal") || kind.contains("approval") {
        TuiActivityKind::Approval
    } else if kind.contains("cancel") {
        TuiActivityKind::Cancellation
    } else if kind.contains("run") || kind.contains("workflow") {
        TuiActivityKind::Run
    } else if kind.contains("chat") || kind.contains("llm") {
        TuiActivityKind::Chat
    } else {
        TuiActivityKind::System
    }
}

pub(super) fn format_proposal_list(proposals: &[ProposalEnvelope]) -> String {
    if proposals.is_empty() {
        return "No proposals found.".to_owned();
    }
    let summary = proposal_list_summary(proposals);
    let mut lines = vec![format!(
        "Proposals: {} total, {} pending, {} approved, {} denied",
        summary.total_count, summary.pending_count, summary.approved_count, summary.denied_count
    )];
    for proposal in proposals {
        lines.push(format!(
            "- {} [{}] {} {} - {}",
            proposal.proposal_id.0,
            proposal_status_label(&proposal.status),
            proposal.kind,
            proposal.agent_id,
            compact_description(&proposal.summary)
        ));
    }
    lines.push(
        "Use /proposal <id>, /approve-proposal <id> [note], or /deny-proposal <id> [note]."
            .to_owned(),
    );
    lines.join("\n")
}

pub(super) fn format_cancel_result(result: &TuiCancelRunResult) -> String {
    if result.cancellation_requested {
        format!(
            "Cancellation requested for run {}. The active runner will stop when it observes the persisted intent.",
            result.run_id.0
        )
    } else {
        format!(
            "Run {} was not cancelled: {} (status {}).",
            result.run_id.0,
            result.message,
            status_label(&result.status)
        )
    }
}

pub(super) fn format_run_list(runs: &[agent_core::AgentRunRecord]) -> String {
    if runs.is_empty() {
        return "No runs found.".to_owned();
    }
    let mut lines = vec![format!("Runs: {} shown", runs.len())];
    for run in runs {
        let finished = run
            .finished_at
            .map(|finished_at| format!(" finished={finished_at}"))
            .unwrap_or_default();
        let cancellation = if run.cancellation_requested() {
            " cancel_requested"
        } else {
            ""
        };
        lines.push(format!(
            "- {} [{}] {} started={}{}{}",
            run.run_id.0,
            status_label(&run.status),
            run.agent_id,
            run.started_at,
            finished,
            cancellation
        ));
    }
    lines.push("Use /inspect <run_id>, /events <run_id>, or /cancel <run_id>.".to_owned());
    lines.join("\n")
}

pub(super) fn format_tui_status(state: &TuiState) -> String {
    let mut lines = vec![
        "TUI status".to_owned(),
        format!(
            "Agent: {} ({} available)",
            state.active_agent_label(),
            state.agents.len()
        ),
        format!(
            "Chat: {} / {}",
            state.options.chat.provider, state.options.chat.model
        ),
        format!("Status line: {}", state.status),
    ];
    if let Some(summary) = &state.catalog_summary {
        lines.push(format!(
            "Catalog: {} agents / {} tools / {} proposal kinds",
            summary.agent_count, summary.tool_count, summary.proposal_kind_count
        ));
    } else {
        lines.push("Catalog: not loaded".to_owned());
    }
    if let Some(inventory) = &state.tool_inventory {
        lines.push(format!(
            "Tools: {} total, {} high-risk, {} blocked",
            inventory.total_count(),
            inventory.high_risk_count(),
            inventory.blocked_count()
        ));
    }
    match (&state.trace_label, &state.trace) {
        (Some(label), Some(trace)) => lines.push(format!(
            "Trace: {} run={} events={}",
            compact_description(label),
            trace.run_id.0,
            trace.events.len()
        )),
        (Some(label), None) => lines.push(format!("Trace: {}", compact_description(label))),
        (None, Some(trace)) => lines.push(format!(
            "Trace: run={} events={}",
            trace.run_id.0,
            trace.events.len()
        )),
        (None, None) => lines.push("Trace: not loaded".to_owned()),
    }
    lines.push(format!("Recent runs: {}", state.recent_runs.len()));
    for run in state.recent_runs.iter().take(3) {
        lines.push(format!(
            "- {} [{}] {}",
            run.run_id.0,
            status_label(&run.status),
            run.agent_id
        ));
    }
    if let Some(run) = &state.latest_run {
        lines.push(format!("Latest run: {} [{}]", run.run_id, run.status));
    }
    if let Some(workflow) = &state.latest_workflow {
        lines.push(format!(
            "Latest workflow: {} [{}] nodes={}",
            workflow.workflow_id, workflow.status, workflow.node_count
        ));
    }
    if let Some(proposals) = &state.latest_proposals {
        lines.push(format!(
            "Latest proposals: {} total, {} pending",
            proposals.total_count, proposals.pending_count
        ));
    }
    if let Some(events) = &state.latest_events {
        lines.push(format!(
            "Latest events: {} showing {}/{}",
            events.run_id, events.shown_count, events.event_count
        ));
    }
    let has_pending_approval = state.pending_approval.is_some();
    if let Some(approval) = &state.pending_approval {
        lines.push(format!("Pending approval: {}", approval.summary()));
    }
    if has_pending_approval {
        lines.push("Next: Tab/Left/Right selects Approve or Deny; Enter confirms".to_owned());
    } else {
        lines.push("Next: /help <command>, /runs, /inspect, /events, /proposals".to_owned());
    }
    lines.join("\n")
}

pub(super) fn run_summary(record: &AgentRunRecord) -> TuiRunSummary {
    TuiRunSummary {
        run_id: record.run_id.0.clone(),
        agent_id: record.agent_id.clone(),
        status: status_label(&record.status).to_owned(),
        started_at: record.started_at.to_string(),
        finished_at: record.finished_at.map(|value| value.to_string()),
        cancellation_requested: record.cancellation_requested(),
        error: record
            .error
            .as_ref()
            .map(|error| format!("{}: {}", error.code, error.message)),
        input_preview: compact_description(&compact_json(&record.input)),
        output_preview: compact_description(&compact_json(&record.output)),
    }
}

pub(super) fn format_run_summary(summary: &TuiRunSummary) -> String {
    let mut lines = vec![
        format!("Run {}: {}", summary.run_id, summary.status),
        format!("Agent: {}", summary.agent_id),
        format!("Started: {}", summary.started_at),
    ];
    if let Some(finished_at) = &summary.finished_at {
        lines.push(format!("Finished: {finished_at}"));
    }
    if summary.cancellation_requested {
        lines.push("Cancellation: requested".to_owned());
    }
    if let Some(error) = &summary.error {
        lines.push(format!("Error: {error}"));
    }
    lines.push(format!("Input: {}", summary.input_preview));
    lines.push(format!("Output: {}", summary.output_preview));
    lines.push(format!(
        "Next: /events {}, /proposals {}, /cancel {}",
        summary.run_id, summary.run_id, summary.run_id
    ));
    lines.join("\n")
}

pub(super) async fn read_workflow_request(path: Utf8PathBuf) -> Result<WorkflowRunRequest> {
    let value = read_json_file(path.clone()).await?;
    validate_workflow_request(&value)?;
    serde_json::from_value::<WorkflowRunRequest>(value)
        .map_err(|e| miette!("failed to parse workflow request at {path}: {e}"))
}

pub(super) fn validate_workflow_request(value: &Value) -> Result<()> {
    let schema = serde_json::from_str::<Value>(include_str!(
        "../../../../../schemas/workflow-run-request.schema.json"
    ))
    .into_diagnostic()?;
    let validator = jsonschema::validator_for(&schema)
        .map_err(|e| miette!("failed to compile workflow-run-request schema: {e}"))?;
    let errors = validator
        .iter_errors(value)
        .map(|error| format!("{}: {}", error.instance_path(), error))
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(miette!(
            "workflow request failed schema validation: {}",
            errors.join("; ")
        ))
    }
}

pub(super) fn format_workflow_result(result: &WorkflowRunResult) -> String {
    let mut lines = vec![format!(
        "Workflow {}: {}",
        result.workflow_id,
        status_label(&result.status)
    )];
    if let Some(root_run_id) = &result.root_run_id {
        lines.push(format!("Root run: {}", root_run_id.0));
    }
    lines.push(format!("Nodes: {}", result.nodes.len()));
    for node in &result.nodes {
        lines.extend(format_workflow_node(node));
    }
    lines.join("\n")
}

pub(super) fn workflow_summary_from_result(result: &WorkflowRunResult) -> TuiWorkflowSummary {
    TuiWorkflowSummary {
        workflow_id: result.workflow_id.clone(),
        status: status_label(&result.status).to_owned(),
        node_count: result.nodes.len(),
        completed_count: result
            .nodes
            .iter()
            .filter(|node| node.status == AgentRunStatus::Completed)
            .count(),
        failed_count: result
            .nodes
            .iter()
            .filter(|node| workflow_status_failed(&node.status))
            .count(),
        skipped_count: result
            .nodes
            .iter()
            .filter(|node| node.status == AgentRunStatus::Skipped)
            .count(),
        compensation_count: result
            .nodes
            .iter()
            .filter(|node| node.compensation.is_some())
            .count(),
        nodes: result.nodes.iter().map(workflow_node_summary).collect(),
    }
}

pub(super) fn workflow_node_summary(node: &WorkflowRunNodeResult) -> TuiWorkflowNodeSummary {
    TuiWorkflowNodeSummary {
        node_id: node.node_id.clone(),
        agent_id: node.agent_id.clone(),
        status: status_label(&node.status).to_owned(),
        run_id: node.run_id.as_ref().map(|run_id| run_id.0.clone()),
        depends_on: node.depends_on.clone(),
        reason: node
            .metadata
            .get("reason")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        blocked_dependencies: node
            .metadata
            .get("blocked_dependencies")
            .and_then(Value::as_array)
            .map(|dependencies| {
                dependencies
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        compensation: node.compensation.as_ref().map(|compensation| {
            TuiWorkflowCompensationSummary {
                agent_id: compensation.agent_id.clone(),
                status: status_label(&compensation.status).to_owned(),
                run_id: compensation.run_id.as_ref().map(|run_id| run_id.0.clone()),
                error: compensation
                    .error
                    .as_ref()
                    .map(|error| format!("{}: {}", error.code, error.message)),
            }
        }),
    }
}

pub(super) fn format_workflow_node(node: &WorkflowRunNodeResult) -> Vec<String> {
    let mut lines = Vec::new();
    let run_id = node
        .run_id
        .as_ref()
        .map(|run_id| run_id.0.as_str())
        .unwrap_or("-");
    let dependencies = if node.depends_on.is_empty() {
        String::new()
    } else {
        format!(" deps={}", node.depends_on.join(","))
    };
    let reason = workflow_result_reason(&node.metadata);
    lines.push(format!(
        "- {} -> {} [{}] run={}{}{}",
        node.node_id,
        node.agent_id,
        status_label(&node.status),
        run_id,
        dependencies,
        reason
    ));
    if let Some(error) = &node.error {
        lines.push(format!("  error {}: {}", error.code, error.message));
    }
    if let Some(compensation) = &node.compensation {
        lines.push(format_workflow_compensation(compensation));
    }
    lines
}

pub(super) fn format_workflow_compensation(
    compensation: &WorkflowRunNodeCompensationResult,
) -> String {
    let run_id = compensation
        .run_id
        .as_ref()
        .map(|run_id| run_id.0.as_str())
        .unwrap_or("-");
    let mut line = format!(
        "  compensation -> {} [{}] run={}",
        compensation.agent_id,
        status_label(&compensation.status),
        run_id
    );
    if let Some(error) = &compensation.error {
        line.push_str(&format!(" error {}: {}", error.code, error.message));
    }
    line
}

pub(super) fn workflow_result_reason(metadata: &Value) -> String {
    let Some(reason) = metadata.get("reason").and_then(Value::as_str) else {
        return String::new();
    };
    let blocked = metadata
        .get("blocked_dependencies")
        .and_then(Value::as_array)
        .map(|dependencies| {
            dependencies
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
        .filter(|dependencies| !dependencies.is_empty());
    match blocked {
        Some(blocked) => format!(" reason={reason} blocked={blocked}"),
        None => format!(" reason={reason}"),
    }
}

pub(super) fn status_label(status: &AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Running => "running",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Skipped => "skipped",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Cancelled => "cancelled",
        AgentRunStatus::TimedOut => "timed_out",
        AgentRunStatus::Abandoned => "abandoned",
    }
}

pub(super) fn proposal_status_label(status: &ProposalStatus) -> &'static str {
    match status {
        ProposalStatus::Created => "created",
        ProposalStatus::PendingApproval => "pending_approval",
        ProposalStatus::Approved => "approved",
        ProposalStatus::Denied => "denied",
        ProposalStatus::Expired => "expired",
        ProposalStatus::Applying => "applying",
        ProposalStatus::Applied => "applied",
        ProposalStatus::ApplyFailed => "apply_failed",
        ProposalStatus::Undoing => "undoing",
        ProposalStatus::Undone => "undone",
        ProposalStatus::UndoFailed => "undo_failed",
    }
}

pub(super) fn proposal_pending(status: &ProposalStatus) -> bool {
    matches!(
        status,
        ProposalStatus::Created | ProposalStatus::PendingApproval
    )
}

pub(super) fn approval_decision_label(decision: &ApprovalDecisionKind) -> &'static str {
    match decision {
        ApprovalDecisionKind::Approve => "approve",
        ApprovalDecisionKind::Deny => "deny",
    }
}

pub(super) fn approval_decision_past_tense(decision: &ApprovalDecisionKind) -> &'static str {
    match decision {
        ApprovalDecisionKind::Approve => "approved",
        ApprovalDecisionKind::Deny => "denied",
    }
}

pub(super) fn workflow_status_failed(status: &AgentRunStatus) -> bool {
    matches!(
        status,
        AgentRunStatus::Failed
            | AgentRunStatus::TimedOut
            | AgentRunStatus::Cancelled
            | AgentRunStatus::Abandoned
    )
}

pub(super) fn format_agent_list(agents: &[agent_core::AgentSpec], active_agent_id: &str) -> String {
    if agents.is_empty() {
        return "No agents are available.".to_owned();
    }

    let mut lines = vec![format!("Active agent: {active_agent_id}")];
    for agent in agents {
        let marker = if agent.id == active_agent_id {
            "*"
        } else {
            " "
        };
        let description = agent
            .description
            .as_deref()
            .map(compact_description)
            .filter(|description| !description.is_empty());
        let detail = match description {
            Some(description) => format!(" - {description}"),
            None => String::new(),
        };
        lines.push(format!("{marker} {} ({}){detail}", agent.id, agent.name));
    }
    lines.push("Use /use <agent_id> to switch the chat target.".to_owned());
    lines.join("\n")
}

pub(super) fn format_tool_inventory(inventory: &TuiToolInventory) -> String {
    if inventory.items.is_empty() {
        return "No tools are available.".to_owned();
    }

    let mut lines = vec![format!(
        "Available tools: {} total, {} high-risk, {} blocked",
        inventory.total_count(),
        inventory.high_risk_count(),
        inventory.blocked_count()
    )];
    for item in &inventory.items {
        lines.push(format!(
            "- {} [{} / {} / {}] {}",
            item.name,
            item.risk.label(),
            item.status_label(),
            item.source,
            compact_description(&item.description),
        ));
    }
    lines.join("\n")
}

pub(super) fn compact_description(description: &str) -> String {
    let compact = description.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_DESCRIPTION: usize = 96;
    if compact.chars().count() > MAX_DESCRIPTION {
        let mut truncated = compact
            .chars()
            .take(MAX_DESCRIPTION.saturating_sub(3))
            .collect::<String>();
        truncated.push_str("...");
        truncated
    } else {
        compact
    }
}
