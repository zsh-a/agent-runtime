use agent_core::{
    AgentProposalStore, AgentRunRecord, AgentRunStatus, AgentRunStore, AgentTrace,
    ApprovalDecisionKind, ApprovalLevel, ProposalEnvelope, ProposalId, ProposalStatus, RunId,
    TraceEvent, WorkflowRunNodeCompensationResult, WorkflowRunNodeResult, WorkflowRunRequest,
    WorkflowRunResult,
};
use agent_store::{FileProposalStore, FileRunStore, FileTraceStore};
use camino::Utf8PathBuf;
use miette::{IntoDiagnostic, Result, miette};
use serde_json::{Value, json};

use crate::proposal::{
    ProposalDecisionInput, append_proposal_decision_trace_event, decide_proposal_with_store,
};
use crate::trace_store::{read_json as read_json_file, read_store_trace};

use super::{
    approval::{
        approve_pending_tool_with_display, call_tool_or_request_approval,
        deny_pending_tool_with_display,
    },
    chat::run_natural_language_command,
    data::{
        TuiActivityItem, TuiActivityKind, TuiProposalListSummary, TuiProposalSummary,
        TuiRunSummary, TuiState, TuiTraceEventItem, TuiTraceEventSummary,
        TuiWorkflowCompensationSummary, TuiWorkflowNodeSummary, TuiWorkflowSummary, read_trace,
    },
    format::{compact_json, pretty_json},
    runtime::{TuiCancelRunResult, TuiRuntime},
    tool_inventory::{TuiToolInventory, load_tui_tool_inventory},
};

pub(super) async fn execute_command(state: &mut TuiState, input: &str) -> Result<()> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(());
    }
    let Some(command) = input.strip_prefix('/') else {
        if state.pending_approval.is_some() {
            match pending_approval_reply(input) {
                Some(PendingApprovalReply::Approve) => {
                    return approve_pending_tool_with_display(state, input).await;
                }
                Some(PendingApprovalReply::Deny) => {
                    return deny_pending_tool_with_display(state, input).await;
                }
                None => {}
            }
        }
        return run_natural_language_command(state, input).await;
    };
    execute_slash_command(state, command.trim()).await
}

async fn execute_slash_command(state: &mut TuiState, input: &str) -> Result<()> {
    let (verb, rest) = split_once(input);
    match verb {
        "" => show_help(state, ""),
        "help" | "?" => show_help(state, rest),
        "clear" => state.clear_output(),
        "status" => show_status_command(state),
        "refresh" => {
            state.refresh().await?;
            state.push_activity(TuiActivityItem::new(
                TuiActivityKind::System,
                "refreshed catalog/trace/store",
            ));
        }
        "agents" => show_agents_command(state).await?,
        "use" => use_agent_command(state, rest).await?,
        "tools" => show_tools_command(state).await?,
        "run" => run_agent_command(state, rest).await?,
        "runs" => list_runs_command(state, rest).await?,
        "cancel" => cancel_run_command(state, rest).await?,
        "workflow" | "wf" => run_workflow_command(state, rest).await?,
        "events" => load_run_events_command(state, rest).await?,
        "proposals" => list_proposals_command(state, rest).await?,
        "proposal" => inspect_proposal_command(state, rest).await?,
        "approve-proposal" => {
            decide_proposal_command(state, rest, ApprovalDecisionKind::Approve).await?
        }
        "deny-proposal" => decide_proposal_command(state, rest, ApprovalDecisionKind::Deny).await?,
        "tool" | "call" => tool_call_command(state, rest).await?,
        "approve" | "yes" | "y" => {
            approve_pending_tool_with_display(state, format!("/{verb}")).await?
        }
        "deny" | "no" | "n" => deny_pending_tool_with_display(state, format!("/{verb}")).await?,
        "trace" | "replay" => load_trace_command(state, rest).await?,
        "inspect" => inspect_run_command(state, rest).await?,
        other => state.push_system_message(unknown_command_message(other)),
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingApprovalReply {
    Approve,
    Deny,
}

fn pending_approval_reply(input: &str) -> Option<PendingApprovalReply> {
    match input.trim().to_ascii_lowercase().as_str() {
        "approve" | "ok" | "y" | "yes" => Some(PendingApprovalReply::Approve),
        "cancel" | "deny" | "n" | "no" => Some(PendingApprovalReply::Deny),
        _ => None,
    }
}

const PRIMARY_COMMANDS: &[&str] = &[
    "help",
    "status",
    "agents",
    "use",
    "tools",
    "run",
    "runs",
    "cancel",
    "workflow",
    "events",
    "proposals",
    "proposal",
    "approve-proposal",
    "deny-proposal",
    "tool",
    "approve",
    "deny",
    "replay",
    "inspect",
    "refresh",
    "clear",
];

const COMMAND_ALIASES: &[(&str, &str)] = &[
    ("?", "help"),
    ("wf", "workflow"),
    ("call", "tool"),
    ("trace", "replay"),
    ("yes", "approve"),
    ("y", "approve"),
    ("no", "deny"),
    ("n", "deny"),
];

fn unknown_command_message(command: &str) -> String {
    let mut message = format!("unknown command '/{command}'.");
    if let Some(suggestion) = suggested_command(command) {
        message.push_str(&format!(" Did you mean /{suggestion}?"));
    }
    message.push_str(&format!(
        " Try: {}",
        PRIMARY_COMMANDS
            .iter()
            .map(|command| format!("/{command}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    message
}

fn suggested_command(command: &str) -> Option<&'static str> {
    if let Some((_, canonical)) = COMMAND_ALIASES.iter().find(|(alias, _)| *alias == command) {
        return Some(*canonical);
    }
    PRIMARY_COMMANDS
        .iter()
        .copied()
        .filter_map(|candidate| {
            let distance = command_distance(command, candidate);
            (candidate.starts_with(command)
                || command.starts_with(candidate)
                || distance <= command_distance_threshold(command, candidate))
            .then_some((candidate, distance))
        })
        .min_by_key(|(candidate, distance)| (*distance, candidate.len()))
        .map(|(candidate, _)| candidate)
}

fn command_distance_threshold(left: &str, right: &str) -> usize {
    if left.len().max(right.len()) <= 5 {
        1
    } else {
        2
    }
}

fn command_distance(left: &str, right: &str) -> usize {
    let right_len = right.chars().count();
    let mut previous = (0..=right_len).collect::<Vec<_>>();
    let mut current = vec![0; right_len + 1];
    for (left_index, left_char) in left.chars().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right.chars().enumerate() {
            let substitution = previous[right_index] + usize::from(left_char != right_char);
            let insertion = current[right_index] + 1;
            let deletion = previous[right_index + 1] + 1;
            current[right_index + 1] = substitution.min(insertion).min(deletion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right_len]
}

fn show_help(state: &mut TuiState, topic: &str) {
    let topic = topic.trim().trim_start_matches('/');
    if !topic.is_empty() {
        state.push_system_message(command_help(topic));
        return;
    }
    state.push_system_message(
        "Type natural language and press Enter to chat with the selected agent.\n\n\
        Slash commands:\n\
        /agents                     list agents and show the active chat target\n\
        /status                     show a compact TUI state summary\n\
        /use <agent_id>              switch the active natural-language agent\n\
        /tools                      list chat tools, risks, sources, and policy status\n\
        /run <agent_id> [json|text]  run a specific runtime agent\n\
        /runs [limit]                list recent persisted runs\n\
        /cancel <run_id>             request cancellation for a running run\n\
        /workflow <path>             run a workflow request JSON file\n\
        /events [run_id] [limit]     show recent trace events for a run\n\
        /proposals [run_id]          list proposals and update the side panel\n\
        /proposal [proposal_id]       inspect one proposal as JSON\n\
        /approve-proposal <id> [note] approve a pending proposal\n\
        /deny-proposal <id> [note]    deny a pending proposal\n\
        /tool <name> [json]          call a tool through active CLI services\n\
        /approve, /yes, /y           approve the pending high-risk tool call\n\
        /deny, /no, /n               deny the pending high-risk tool call\n\
        /replay <trace_path>         load a trace into the side panel\n\
        /inspect [run_id]            load a persisted run record summary\n\
        /refresh                     reload catalog, trace, and recent runs\n\
        /clear                       clear chat and activity\n\n\
        Keys:\n\
        Enter sends, Shift+Enter inserts a newline, Esc/Ctrl-C cancels a running task\n\
        Left/Right move the cursor, Ctrl/Alt+Left/Right move by word\n\
        Ctrl+A/E jump to start/end, Ctrl+U/K delete before/after cursor\n\
        Ctrl+W deletes the previous word, Up/Down browse input history\n\
        PageUp/PageDown scroll chat, Tab completes slash commands\n\
        When approval is pending, Tab/Left/Right selects Approve or Deny and Enter confirms\n\n\
        Use /help <command> for focused help, for example /help events.",
    );
}

fn command_help(topic: &str) -> String {
    match topic {
        "agents" | "use" => "Agent selection\n\n\
            /agents\n\
            List available agents and show the current chat target.\n\n\
            /use <agent_id>\n\
            Switch the active natural-language chat agent. Press Tab after /use to complete an agent id."
            .to_owned(),
        "status" => "Status\n\n\
            /status\n\
            Show the current TUI state in Chat: active agent, model, tools, trace, recent runs, latest run/proposals/events, and pending approval."
            .to_owned(),
        "run" => "Run an agent\n\n\
            /run <agent_id> [json|text]\n\
            Execute a runtime agent once. JSON input is passed through; plain text becomes {\"message\": text}.\n\n\
            Examples:\n\
            /run echo_agent hello\n\
            /run echo_agent {\"message\":\"hello\"}\n\n\
            Next steps: /inspect, /events, /proposals."
            .to_owned(),
        "runs" | "inspect" => "Runs\n\n\
            /runs [limit]\n\
            List recent persisted runs and update the Activity panel.\n\n\
            /inspect [run_id]\n\
            Show a compact run summary. If run_id is omitted, TUI uses the current trace, latest inspected run, or newest recent run.\n\n\
            Press Tab after /inspect to complete a recent run id."
            .to_owned(),
        "events" => "Trace events\n\n\
            /events [run_id] [limit]\n\
            Load recent persisted trace events into Chat, Activity, and Context. Defaults to the current run and 12 events.\n\n\
            Examples:\n\
            /events\n\
            /events 20\n\
            /events run_123 20"
            .to_owned(),
        "workflow" | "wf" => "Workflow\n\n\
            /workflow <path>\n\
            /wf <path>\n\
            Validate and run a workflow request JSON file, then show node status and write traces."
            .to_owned(),
        "proposals" | "proposal" | "approve-proposal" | "deny-proposal" => "Proposals\n\n\
            /proposals [run_id]\n\
            List proposals, optionally filtered by run.\n\n\
            /proposal [proposal_id]\n\
            Inspect one proposal as JSON. If proposal_id is omitted, TUI uses the single proposal currently shown in Context.\n\n\
            /approve-proposal <id> [note]\n\
            /deny-proposal <id> [note]\n\
            Decide a pending proposal and append a trace event.\n\n\
            Press Tab after proposal commands to complete known ids."
            .to_owned(),
        "tool" | "call" | "approve" | "yes" | "y" | "deny" | "no" | "n" => "Tools and approval\n\n\
            /tool <name> [json]\n\
            /call <name> [json]\n\
            Call a tool through active CLI services. High-risk tools pause for approval.\n\n\
            When an approval card is shown, use Tab/Left/Right to select Approve or Deny, then press Enter.\n\
            You can also type yes/no or use /approve, /yes, /deny, /no.\n\n\
            Press Tab after /tool to complete a tool name."
            .to_owned(),
        "cancel" => "Cancellation\n\n\
            /cancel <run_id>\n\
            Persist cancellation intent for a running run. A run id is always required to avoid accidental cancellation."
            .to_owned(),
        "trace" | "replay" => "Trace replay\n\n\
            /replay <trace_path>\n\
            /trace <trace_path>\n\
            Load a trace JSON file into the side panel without executing it."
            .to_owned(),
        "refresh" | "clear" => "Session controls\n\n\
            /refresh\n\
            Reload catalog, tools, trace, and recent runs.\n\n\
            /clear\n\
            Clear chat, activity, latest summaries, and pending approval state."
            .to_owned(),
        _ => "No focused help found for that command.\n\n\
            Try /help run, /help events, /help proposals, /help tool, /help inspect, or /help workflow."
            .to_owned(),
    }
}

fn show_status_command(state: &mut TuiState) {
    state.push_user_message("/status");
    state.push_activity(TuiActivityItem::new(
        TuiActivityKind::System,
        "status shown",
    ));
    state.push_system_message(format_tui_status(state));
}

async fn show_agents_command(state: &mut TuiState) -> Result<()> {
    let runtime = TuiRuntime::load(&state.options).await?;
    let active_agent_id = runtime.resolve_agent_id(state.selected_agent_id.as_deref())?;
    state.push_user_message("/agents");
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::System,
        "agents listed",
        format!("{} available", runtime.agent_specs().len()),
    ));
    state.push_system_message(format_agent_list(runtime.agent_specs(), &active_agent_id));
    Ok(())
}

async fn use_agent_command(state: &mut TuiState, rest: &str) -> Result<()> {
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

async fn show_tools_command(state: &mut TuiState) -> Result<()> {
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
    Ok(())
}

async fn run_agent_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let (agent_id, json_input) = split_name_and_json(rest, "agent id")?;
    let input = parse_run_input(json_input)?;
    state.push_user_message(format!("/run {agent_id} {}", compact_json(&input)));
    run_agent_with_input(state, &agent_id, input, "slash_command").await
}

async fn list_runs_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let limit = run_list_limit(rest)?;
    let runs = load_runs(&state.options.store_path, limit).await?;
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
    Ok(())
}

async fn cancel_run_command(state: &mut TuiState, rest: &str) -> Result<()> {
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

async fn run_workflow_command(state: &mut TuiState, rest: &str) -> Result<()> {
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

async fn load_run_events_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let (run_id, limit) = run_events_args(state, rest)?;
    let trace = load_store_trace(&state.options.store_path, &run_id).await?;
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

async fn list_proposals_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let run_id = optional_run_id(rest)?;
    let proposals = load_proposals(&state.options.store_path, run_id.as_ref()).await?;
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

async fn inspect_proposal_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let proposal_id = proposal_id_arg_or_default(state, rest)?;
    let proposal = load_proposal(&state.options.store_path, &proposal_id).await?;
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

async fn decide_proposal_command(
    state: &mut TuiState,
    rest: &str,
    decision: ApprovalDecisionKind,
) -> Result<()> {
    let (proposal_id, comment) = proposal_decision_args(rest)?;
    let store_path = state.options.store_path.clone();
    let store = FileProposalStore::new(store_path.clone())
        .await
        .into_diagnostic()?;
    let trace_store = FileTraceStore::new(store_path).await.into_diagnostic()?;
    let mut proposal = store
        .get_proposal(&proposal_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("proposal '{}' was not found", proposal_id.0))?;
    let response = decide_proposal_with_store(
        &store,
        &mut proposal,
        ProposalDecisionInput {
            decision,
            approval_level: Some(ApprovalLevel::SingleUser),
            decided_by: Some("agent_tui".to_owned()),
            comment,
        },
    )
    .await?;
    append_proposal_decision_trace_event(&trace_store, &response).await?;
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

async fn run_agent_with_input(
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

async fn tool_call_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let (name, json_input) = split_name_and_json(rest, "tool name")?;
    let input = parse_json_or_default(json_input, "tool input")?;
    state.push_user_message(format!("/tool {name} {}", compact_json(&input)));
    call_tool_or_request_approval(state, name, input).await
}

async fn load_trace_command(state: &mut TuiState, rest: &str) -> Result<()> {
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

async fn load_store_trace(store_path: &Utf8PathBuf, run_id: &RunId) -> Result<AgentTrace> {
    let value = read_store_trace(store_path, run_id)
        .await?
        .ok_or_else(|| miette!("trace for run '{}' was not found", run_id.0))?;
    serde_json::from_value(value)
        .map_err(|e| miette!("failed to parse trace for run '{}': {e}", run_id.0))
}

async fn inspect_run_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let run_id = run_id_arg_or_default(state, rest)?;
    let store = FileRunStore::new(state.options.store_path.clone())
        .await
        .into_diagnostic()?;
    let record = store
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

fn split_once(input: &str) -> (&str, &str) {
    input
        .trim()
        .split_once(char::is_whitespace)
        .map(|(head, tail)| (head, tail.trim()))
        .unwrap_or((input.trim(), ""))
}

fn split_name_and_json<'a>(input: &'a str, label: &str) -> Result<(String, &'a str)> {
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

fn parse_json_or_default(input: &str, label: &str) -> Result<Value> {
    if input.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(input).map_err(|e| miette!("failed to parse {label} as JSON: {e}"))
}

fn parse_run_input(input: &str) -> Result<Value> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_str(input) {
        Ok(value) => Ok(value),
        Err(_) => Ok(json!({"message": input})),
    }
}

fn push_agent_output(state: &mut TuiState, output: &Value) {
    if let Some(message) = output.get("message").and_then(Value::as_str) {
        state.push_assistant_message(message.to_owned());
    } else if let Some(content) = output.get("content").and_then(Value::as_str) {
        state.push_assistant_message(content.to_owned());
    } else {
        state.push_assistant_message(pretty_json(output));
    }
}

async fn load_proposals(
    store_path: &Utf8PathBuf,
    run_id: Option<&RunId>,
) -> Result<Vec<ProposalEnvelope>> {
    let store = FileProposalStore::new(store_path.clone())
        .await
        .into_diagnostic()?;
    store.list_proposals(run_id).await.into_diagnostic()
}

async fn load_proposal(
    store_path: &Utf8PathBuf,
    proposal_id: &ProposalId,
) -> Result<ProposalEnvelope> {
    let store = FileProposalStore::new(store_path.clone())
        .await
        .into_diagnostic()?;
    store
        .get_proposal(proposal_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("proposal '{}' was not found", proposal_id.0))
}

async fn load_runs(
    store_path: &Utf8PathBuf,
    limit: usize,
) -> Result<Vec<agent_core::AgentRunRecord>> {
    let store = FileRunStore::new(store_path.clone())
        .await
        .into_diagnostic()?;
    store.list_runs(None, Some(limit)).await.into_diagnostic()
}

fn optional_run_id(input: &str) -> Result<Option<RunId>> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    if input.split_whitespace().count() > 1 {
        return Err(miette!("expected at most one run_id"));
    }
    Ok(Some(RunId(input.to_owned())))
}

fn run_list_limit(input: &str) -> Result<usize> {
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

fn run_id_arg(input: &str) -> Result<RunId> {
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

fn run_id_arg_or_default(state: &TuiState, input: &str) -> Result<RunId> {
    let input = input.trim();
    if input.is_empty() {
        return default_run_id(state);
    }
    run_id_arg(input)
}

fn default_run_id(state: &TuiState) -> Result<RunId> {
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

fn run_events_args(state: &TuiState, input: &str) -> Result<(RunId, usize)> {
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

fn proposal_id_arg(input: &str) -> Result<ProposalId> {
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

fn proposal_id_arg_or_default(state: &TuiState, input: &str) -> Result<ProposalId> {
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

fn proposal_decision_args(input: &str) -> Result<(ProposalId, Option<String>)> {
    let input = input.trim();
    if input.is_empty() {
        return Err(miette!("proposal id is required"));
    }
    let (proposal_id, comment) = split_once(input);
    let comment = (!comment.is_empty()).then(|| comment.to_owned());
    Ok((ProposalId(proposal_id.to_owned()), comment))
}

fn proposal_list_summary(proposals: &[ProposalEnvelope]) -> TuiProposalListSummary {
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

fn proposal_summary(proposal: &ProposalEnvelope) -> TuiProposalSummary {
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

fn trace_event_summary(trace: &AgentTrace, limit: usize) -> TuiTraceEventSummary {
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

fn trace_event_item(event: &TraceEvent) -> TuiTraceEventItem {
    TuiTraceEventItem {
        kind: event.kind.clone(),
        detail: trace_event_detail(event),
    }
}

fn trace_event_detail(event: &TraceEvent) -> Option<String> {
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

fn compact_event_value(value: &Value) -> String {
    let raw = value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| compact_json(value));
    compact_description(&raw)
}

fn format_trace_event_summary(summary: &TuiTraceEventSummary) -> String {
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

fn activity_kind_from_event(kind: &str) -> TuiActivityKind {
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

fn format_proposal_list(proposals: &[ProposalEnvelope]) -> String {
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

fn format_cancel_result(result: &TuiCancelRunResult) -> String {
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

fn format_run_list(runs: &[agent_core::AgentRunRecord]) -> String {
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

fn format_tui_status(state: &TuiState) -> String {
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

fn run_summary(record: &AgentRunRecord) -> TuiRunSummary {
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

fn format_run_summary(summary: &TuiRunSummary) -> String {
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

async fn read_workflow_request(path: Utf8PathBuf) -> Result<WorkflowRunRequest> {
    let value = read_json_file(path.clone()).await?;
    validate_workflow_request(&value)?;
    serde_json::from_value::<WorkflowRunRequest>(value)
        .map_err(|e| miette!("failed to parse workflow request at {path}: {e}"))
}

fn validate_workflow_request(value: &Value) -> Result<()> {
    let schema = serde_json::from_str::<Value>(include_str!(
        "../../../../schemas/workflow-run-request.schema.json"
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

fn format_workflow_result(result: &WorkflowRunResult) -> String {
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

fn workflow_summary_from_result(result: &WorkflowRunResult) -> TuiWorkflowSummary {
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

fn workflow_node_summary(node: &WorkflowRunNodeResult) -> TuiWorkflowNodeSummary {
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

fn format_workflow_node(node: &WorkflowRunNodeResult) -> Vec<String> {
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

fn format_workflow_compensation(compensation: &WorkflowRunNodeCompensationResult) -> String {
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

fn workflow_result_reason(metadata: &Value) -> String {
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

fn status_label(status: &AgentRunStatus) -> &'static str {
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

fn proposal_status_label(status: &ProposalStatus) -> &'static str {
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

fn proposal_pending(status: &ProposalStatus) -> bool {
    matches!(
        status,
        ProposalStatus::Created | ProposalStatus::PendingApproval
    )
}

fn approval_decision_label(decision: &ApprovalDecisionKind) -> &'static str {
    match decision {
        ApprovalDecisionKind::Approve => "approve",
        ApprovalDecisionKind::Deny => "deny",
    }
}

fn approval_decision_past_tense(decision: &ApprovalDecisionKind) -> &'static str {
    match decision {
        ApprovalDecisionKind::Approve => "approved",
        ApprovalDecisionKind::Deny => "denied",
    }
}

fn workflow_status_failed(status: &AgentRunStatus) -> bool {
    matches!(
        status,
        AgentRunStatus::Failed
            | AgentRunStatus::TimedOut
            | AgentRunStatus::Cancelled
            | AgentRunStatus::Abandoned
    )
}

fn format_agent_list(agents: &[agent_core::AgentSpec], active_agent_id: &str) -> String {
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

fn format_tool_inventory(inventory: &TuiToolInventory) -> String {
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

fn compact_description(description: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{
        data::{TranscriptRole, TuiOptions, TuiPendingApproval},
        policy::TuiToolRisk,
        test_support::{test_options, test_state},
    };
    use agent_core::{
        AgentRunRecord, AgentTrace, AgentTraceStore, PROTOCOL_VERSION, RunScope, ToolRisk,
        ToolSpec, TraceEvent,
    };
    use agent_store::FileTraceStore;
    use camino::Utf8PathBuf;
    use time::OffsetDateTime;

    async fn multi_agent_state(dir: &tempfile::TempDir) -> TuiState {
        let registry_path = dir.path().join("multi-agents.yaml");
        fs_err::write(
            &registry_path,
            r#"agents:
  - protocol_version: agent.v1
    id: echo_agent
    name: Echo Agent
    description: First test agent.
    version: 0.1.0
    runner: echo
    schedule:
      type: manual
    capabilities: []
    metadata: {}
  - protocol_version: agent.v1
    id: review_agent
    name: Review Agent
    description: Second test agent.
    version: 0.1.0
    runner: echo
    schedule:
      type: manual
    capabilities: []
    metadata: {}
"#,
        )
        .expect("registry writes");
        let mut options = test_options(dir, "mock response", true);
        options.runtime_sources.registry =
            Utf8PathBuf::from_path_buf(registry_path).expect("registry path is utf8");
        TuiState::load(options).await.expect("state loads")
    }

    fn add_high_risk_echo_tool(options: &mut TuiOptions) {
        options.tool_overrides.source_specs.push(ToolSpec {
            name: "echo".to_owned(),
            description: "High-risk echo test tool.".to_owned(),
            input_schema: json!({"type": "object"}),
            output_schema: Some(json!({"type": "object"})),
            risk: ToolRisk::High,
            metadata: json!({"source": "test_high_risk"}),
        });
    }

    async fn high_risk_echo_state(
        dir: &tempfile::TempDir,
        allow_high_risk_tools: bool,
    ) -> TuiState {
        let mut options = test_options(dir, "mock response", allow_high_risk_tools);
        add_high_risk_echo_tool(&mut options);
        TuiState::load(options).await.expect("state loads")
    }

    async fn write_test_trace(store_path: &Utf8PathBuf, trace: &AgentTrace) {
        let trace_store = FileTraceStore::new(store_path.clone())
            .await
            .expect("trace store loads");
        trace_store
            .write_trace(trace.clone())
            .await
            .expect("trace writes");
    }

    #[tokio::test]
    async fn help_command_can_show_focused_command_help() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(&mut state, "/help events")
            .await
            .expect("help command succeeds");

        assert!(state.transcript.iter().any(|item| {
            item.content.contains("Trace events")
                && item.content.contains("/events [run_id] [limit]")
                && item
                    .content
                    .contains("Defaults to the current run and 12 events")
        }));
    }

    #[tokio::test]
    async fn help_command_accepts_slash_prefixed_topic() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(&mut state, "/help /tool")
            .await
            .expect("help command succeeds");

        assert!(state.transcript.iter().any(|item| {
            item.content.contains("Tools and approval") && item.content.contains("/tool <name>")
        }));
    }

    #[tokio::test]
    async fn bare_slash_shows_help() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(&mut state, "/")
            .await
            .expect("bare slash shows help");

        assert!(state.transcript.iter().any(|item| {
            item.content.contains("Slash commands:")
                && item
                    .content
                    .contains("Use /help <command> for focused help")
        }));
    }

    #[tokio::test]
    async fn status_command_shows_compact_tui_state() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        create_test_run(
            &state.options.store_path,
            RunId("run_status_completed".to_owned()),
            AgentRunStatus::Completed,
        )
        .await;
        state.refresh_runs().await.expect("runs refresh");

        execute_command(&mut state, "/status")
            .await
            .expect("status command succeeds");

        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::System && activity.title == "status shown"
        }));
        assert!(
            state
                .transcript
                .iter()
                .any(|item| { item.role == TranscriptRole::User && item.content == "/status" })
        );
        assert!(state.transcript.iter().any(|item| {
            item.content.contains("TUI status")
                && item.content.contains("Agent: echo_agent")
                && item.content.contains("Chat: mock / mock-model")
                && item.content.contains("Tools: 1 total")
                && item.content.contains("Recent runs: 1")
                && item.content.contains("run_status_completed [completed]")
                && item.content.contains("Next: /help <command>")
        }));
    }

    #[tokio::test]
    async fn status_command_prioritizes_pending_approval_next_step() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        state.set_pending_approval(TuiPendingApproval::tool_call(
            "shell.exec",
            TuiToolRisk::High,
            json!({}),
        ));

        execute_command(&mut state, "/status")
            .await
            .expect("status command succeeds");

        assert!(state.transcript.iter().any(|item| {
            item.content.contains("Pending approval: shell.exec (high)")
                && item
                    .content
                    .contains("Next: Tab/Left/Right selects Approve or Deny; Enter confirms")
        }));
    }

    #[tokio::test]
    async fn unknown_command_suggests_near_match() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(&mut state, "/event")
            .await
            .expect("unknown command reports help");

        assert!(state.transcript.iter().any(|item| {
            item.content.contains("unknown command '/event'")
                && item.content.contains("Did you mean /events?")
                && item.content.contains("Try: /help")
        }));
    }

    #[tokio::test]
    async fn unknown_command_omits_suggestion_for_distant_match() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(&mut state, "/zzzz")
            .await
            .expect("unknown command reports help");

        let message = state
            .transcript
            .iter()
            .find(|item| item.content.contains("unknown command '/zzzz'"))
            .expect("unknown command message");
        assert!(!message.content.contains("Did you mean"));
        assert!(message.content.contains("Try: /help"));
    }

    #[tokio::test]
    async fn tools_command_lists_runtime_tools_and_policy_status() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(&mut state, "/tools")
            .await
            .expect("tools command succeeds");

        let inventory = state.tool_inventory.as_ref().expect("inventory is loaded");
        assert_eq!(inventory.total_count(), 1);
        assert_eq!(inventory.high_risk_count(), 0);
        assert_eq!(inventory.blocked_count(), 0);
        assert!(state.transcript.iter().any(|item| {
            item.content
                .contains("- echo [read_only / allowed / agent_cli_builtin]")
        }));
        let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
        assert!(rendered.contains("tools 1 high 0 blocked 0"));
    }

    #[tokio::test]
    async fn tools_command_marks_high_risk_tools_blocked_by_policy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = high_risk_echo_state(&dir, false).await;

        execute_command(&mut state, "/tools")
            .await
            .expect("tools command succeeds");

        let inventory = state.tool_inventory.as_ref().expect("inventory is loaded");
        assert_eq!(inventory.blocked_count(), 1);
        assert!(state.transcript.iter().any(|item| {
            item.content
                .contains("- echo [high / blocked / test_high_risk]")
        }));
    }

    #[tokio::test]
    async fn run_command_executes_agent_and_loads_trace() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(
            &mut state,
            r#"/run echo_agent {"message":"from interactive tui"}"#,
        )
        .await
        .expect("run command succeeds");

        assert!(state.trace.is_some());
        assert_eq!(state.recent_runs.len(), 1);
        assert_eq!(state.recent_runs[0].agent_id, "echo_agent");
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("from interactive tui"))
        );
    }

    #[tokio::test]
    async fn natural_language_input_runs_default_agent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "chat answer").await;

        execute_command(&mut state, "Summarize my day")
            .await
            .expect("natural input runs");

        assert!(state.trace.is_none());
        assert!(state.recent_runs.is_empty());
        assert_eq!(state.chat_messages.len(), 2);
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("Summarize my day"))
        );
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("chat answer"))
        );
        let assistant_items = state
            .transcript
            .iter()
            .filter(|item| item.role == TranscriptRole::Assistant)
            .collect::<Vec<_>>();
        assert_eq!(assistant_items.len(), 1);
        assert_eq!(assistant_items[0].content, "chat answer");
    }

    #[tokio::test]
    async fn run_command_accepts_text_input() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(&mut state, "/run echo_agent hello tui")
            .await
            .expect("text run command succeeds");

        assert!(state.trace.is_some());
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("hello tui"))
        );
    }

    #[tokio::test]
    async fn cancel_command_persists_intent_for_running_run() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        let run_id = RunId("run_cancel_running".to_owned());
        create_test_run(
            &state.options.store_path,
            run_id.clone(),
            AgentRunStatus::Running,
        )
        .await;

        execute_command(&mut state, &format!("/cancel {}", run_id.0))
            .await
            .expect("cancel command succeeds");

        let store = FileRunStore::new(state.options.store_path.clone())
            .await
            .expect("run store opens");
        let stored = store
            .get_run(&run_id)
            .await
            .expect("run reads")
            .expect("run exists");
        assert!(stored.cancellation_requested());
        assert_eq!(
            stored.metadata["control"]["cancel_requested_by"],
            "agent_tui"
        );
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Cancellation
                && activity.title == "cancellation requested"
        }));
        assert!(state.transcript.iter().any(|item| {
            item.content
                .contains("Cancellation requested for run run_cancel_running")
        }));
    }

    #[tokio::test]
    async fn cancel_command_reports_non_running_run_without_mutating() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        let run_id = RunId("run_cancel_completed".to_owned());
        create_test_run(
            &state.options.store_path,
            run_id.clone(),
            AgentRunStatus::Completed,
        )
        .await;

        execute_command(&mut state, &format!("/cancel {}", run_id.0))
            .await
            .expect("cancel command succeeds");

        let store = FileRunStore::new(state.options.store_path.clone())
            .await
            .expect("run store opens");
        let stored = store
            .get_run(&run_id)
            .await
            .expect("run reads")
            .expect("run exists");
        assert!(!stored.cancellation_requested());
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Cancellation && activity.title == "run not cancelled"
        }));
        assert!(state.transcript.iter().any(|item| {
            item.content
                .contains("Run run_cancel_completed was not cancelled")
        }));
    }

    #[tokio::test]
    async fn runs_command_lists_recent_runs_and_updates_activity_panel() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        create_test_run(
            &state.options.store_path,
            RunId("run_list_completed".to_owned()),
            AgentRunStatus::Completed,
        )
        .await;
        create_test_run(
            &state.options.store_path,
            RunId("run_list_running".to_owned()),
            AgentRunStatus::Running,
        )
        .await;

        execute_command(&mut state, "/runs 2")
            .await
            .expect("runs command succeeds");

        assert_eq!(state.recent_runs.len(), 2);
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Run
                && activity.title == "runs listed"
                && activity.detail.as_deref() == Some("2 shown")
        }));
        assert!(state.transcript.iter().any(|item| {
            item.content.contains("Runs: 2 shown")
                && item.content.contains("run_list_completed")
                && item.content.contains("run_list_running")
                && item
                    .content
                    .contains("Use /inspect <run_id>, /events <run_id>, or /cancel <run_id>.")
        }));
        let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
        assert!(rendered.contains("recent runs"));
        assert!(rendered.contains("run_list_completed") || rendered.contains("run_list_running"));
    }

    #[tokio::test]
    async fn inspect_command_shows_run_summary_and_updates_context_panel() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        create_test_run(
            &state.options.store_path,
            RunId("run_inspect_summary".to_owned()),
            AgentRunStatus::Completed,
        )
        .await;

        execute_command(&mut state, "/inspect run_inspect_summary")
            .await
            .expect("inspect command succeeds");

        let summary = state.latest_run.as_ref().expect("run summary");
        assert_eq!(summary.run_id, "run_inspect_summary");
        assert_eq!(summary.agent_id, "echo_agent");
        assert_eq!(summary.status, "completed");
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Run
                && activity.title == "run run_inspect_summary"
                && activity.detail.as_deref() == Some("echo_agent completed")
        }));
        assert!(state.transcript.iter().any(|item| {
            item.content.contains("Run run_inspect_summary: completed")
                && item.content.contains("Agent: echo_agent")
                && item.content.contains("Next: /events run_inspect_summary")
        }));
        let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
        assert!(rendered.contains("inspected run"));
        assert!(rendered.contains("run_inspect_summary"));
        assert!(rendered.contains("status completed"));
    }

    #[tokio::test]
    async fn inspect_command_defaults_to_most_recent_run() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        create_test_run(
            &state.options.store_path,
            RunId("run_inspect_default".to_owned()),
            AgentRunStatus::Completed,
        )
        .await;
        state.refresh_runs().await.expect("runs refresh");

        execute_command(&mut state, "/inspect")
            .await
            .expect("inspect command succeeds");

        assert_eq!(
            state.latest_run.as_ref().expect("run summary").run_id,
            "run_inspect_default"
        );
        assert!(
            state
                .transcript
                .iter()
                .any(|item| { item.content.contains("Run run_inspect_default: completed") })
        );
        assert!(state.transcript.iter().any(|item| {
            item.role == TranscriptRole::User && item.content == "/inspect run_inspect_default"
        }));
    }

    #[tokio::test]
    async fn workflow_command_runs_dag_and_shows_node_summary() {
        let dir = tempfile::tempdir().expect("temp dir");
        let workflow_path = dir.path().join("workflow.json");
        fs_err::write(
            &workflow_path,
            r#"{
  "protocol_version": "agent.v1",
  "workflow_id": "tui_workflow_test",
  "nodes": [
    {
      "node_id": "first",
      "agent_id": "echo_agent",
      "input": {"message": "root"}
    },
    {
      "node_id": "second",
      "agent_id": "echo_agent",
      "depends_on": ["first"],
      "input": {"message": "child"},
      "input_mappings": [
        {
          "from_node": "first",
          "from_path": "/message",
          "to_path": "/source/message"
        }
      ]
    }
  ],
  "metadata": {"source": "tui_test"}
}"#,
        )
        .expect("workflow writes");
        let workflow_path =
            Utf8PathBuf::from_path_buf(workflow_path).expect("workflow path is utf8");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(&mut state, &format!("/workflow {workflow_path}"))
            .await
            .expect("workflow command succeeds");

        assert!(state.trace.is_some());
        assert_eq!(state.recent_runs.len(), 2);
        let workflow = state.latest_workflow.as_ref().expect("workflow summary");
        assert_eq!(workflow.workflow_id, "tui_workflow_test");
        assert_eq!(workflow.status, "completed");
        assert_eq!(workflow.node_count, 2);
        assert_eq!(workflow.completed_count, 2);
        assert_eq!(workflow.failed_count, 0);
        assert_eq!(workflow.skipped_count, 0);
        assert_eq!(workflow.nodes[1].depends_on, vec!["first"]);
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Run
                && activity.title == "workflow tui_workflow_test"
                && activity.detail.as_deref() == Some("2 nodes completed")
        }));
        assert!(state.transcript.iter().any(|item| {
            item.content
                .contains("Workflow tui_workflow_test: completed")
                && item.content.contains("- first -> echo_agent [completed]")
                && item.content.contains("- second -> echo_agent [completed]")
                && item.content.contains("deps=first")
        }));
        let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
        assert!(rendered.contains("workflow"));
        assert!(rendered.contains("tui_workflow_test [completed]"));
        assert!(rendered.contains("nodes 2 ok 2 fail 0 skip 0"));
    }

    #[tokio::test]
    async fn workflow_command_validates_schema_before_running() {
        let dir = tempfile::tempdir().expect("temp dir");
        let workflow_path = dir.path().join("invalid-workflow.json");
        fs_err::write(
            &workflow_path,
            r#"{"protocol_version":"agent.v1","workflow_id":"missing_nodes"}"#,
        )
        .expect("workflow writes");
        let workflow_path =
            Utf8PathBuf::from_path_buf(workflow_path).expect("workflow path is utf8");
        let mut state = test_state(&dir, "mock response").await;

        let error = execute_command(&mut state, &format!("/workflow {workflow_path}"))
            .await
            .expect_err("invalid workflow is rejected");

        assert!(
            error
                .to_string()
                .contains("workflow request failed schema validation")
        );
        assert!(state.recent_runs.is_empty());
    }

    #[tokio::test]
    async fn events_command_loads_recent_trace_events_into_activity_and_context() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        let run_id = RunId("run_events_test".to_owned());
        let now = OffsetDateTime::now_utc();
        let trace = AgentTrace {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            runtime_version: "test".to_owned(),
            run_id: run_id.clone(),
            agent_id: "echo_agent".to_owned(),
            agent_version: "0.1.0".to_owned(),
            scope: RunScope::Global,
            started_at: now,
            finished_at: now,
            input: json!({"message": "trace me"}),
            output: json!({"message": "done"}),
            workflow: None,
            usage_summary: None,
            spans: Vec::new(),
            events: vec![
                TraceEvent::new("run_started", json!({"agent_id": "echo_agent"})),
                TraceEvent::new(
                    "tool_call_finished",
                    json!({
                        "tool_name": "echo",
                        "status": "completed",
                        "duration_ms": 7
                    }),
                ),
                TraceEvent::new(
                    "proposal_decided",
                    json!({
                        "proposal_id": "proposal_1",
                        "decision": "approve"
                    }),
                ),
            ],
            artifact_refs: Vec::new(),
        };
        write_test_trace(&state.options.store_path, &trace).await;

        execute_command(&mut state, &format!("/events {} 2", run_id.0))
            .await
            .expect("events command succeeds");

        let events = state.latest_events.as_ref().expect("event summary");
        assert_eq!(events.run_id, "run_events_test");
        assert_eq!(events.agent_id, "echo_agent");
        assert_eq!(events.event_count, 3);
        assert_eq!(events.shown_count, 2);
        assert_eq!(events.events[0].kind, "tool_call_finished");
        assert_eq!(events.events[1].kind, "proposal_decided");
        assert_eq!(
            state.trace.as_ref().expect("trace loaded").run_id.0,
            "run_events_test"
        );
        assert!(
            state
                .trace_label
                .as_deref()
                .is_some_and(|label| label.contains("store trace run_events_test"))
        );
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Tool && activity.title == "tool_call_finished"
        }));
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Approval && activity.title == "proposal_decided"
        }));
        assert!(state.transcript.iter().any(|item| {
            item.content
                .contains("Events for run run_events_test: showing 2/3")
                && item.content.contains("tool_name=echo")
                && item.content.contains("proposal_id=proposal_1")
        }));
        let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
        assert!(rendered.contains("events"));
        assert!(rendered.contains("run_events_test 2/3"));
        assert!(rendered.contains("tool_call_finished"));
    }

    #[tokio::test]
    async fn events_command_defaults_to_current_trace_run() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        let now = OffsetDateTime::now_utc();
        let trace = AgentTrace {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            runtime_version: "test".to_owned(),
            run_id: RunId("run_events_default".to_owned()),
            agent_id: "echo_agent".to_owned(),
            agent_version: "0.1.0".to_owned(),
            scope: RunScope::Global,
            started_at: now,
            finished_at: now,
            input: json!({}),
            output: json!({}),
            workflow: None,
            usage_summary: None,
            spans: Vec::new(),
            events: vec![TraceEvent::new(
                "run_finished",
                json!({"status": "completed"}),
            )],
            artifact_refs: Vec::new(),
        };
        write_test_trace(&state.options.store_path, &trace).await;
        state.set_trace("current trace", trace);

        execute_command(&mut state, "/events")
            .await
            .expect("events command succeeds");

        let events = state.latest_events.as_ref().expect("event summary");
        assert_eq!(events.run_id, "run_events_default");
        assert_eq!(events.shown_count, 1);
        assert!(state.transcript.iter().any(|item| {
            item.role == TranscriptRole::User && item.content == "/events run_events_default 12"
        }));
        assert!(state.transcript.iter().any(|item| {
            item.content
                .contains("Events for run run_events_default: showing 1/1")
        }));
    }

    #[tokio::test]
    async fn events_command_treats_single_numeric_argument_as_default_run_limit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        let now = OffsetDateTime::now_utc();
        let trace = AgentTrace {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            runtime_version: "test".to_owned(),
            run_id: RunId("run_events_limit".to_owned()),
            agent_id: "echo_agent".to_owned(),
            agent_version: "0.1.0".to_owned(),
            scope: RunScope::Global,
            started_at: now,
            finished_at: now,
            input: json!({}),
            output: json!({}),
            workflow: None,
            usage_summary: None,
            spans: Vec::new(),
            events: vec![
                TraceEvent::new("run_started", json!({})),
                TraceEvent::new("tool_call_finished", json!({"tool_name": "echo"})),
                TraceEvent::new("run_finished", json!({"status": "completed"})),
            ],
            artifact_refs: Vec::new(),
        };
        write_test_trace(&state.options.store_path, &trace).await;
        state.set_trace("current trace", trace);

        execute_command(&mut state, "/events 2")
            .await
            .expect("events command succeeds");

        let events = state.latest_events.as_ref().expect("event summary");
        assert_eq!(events.run_id, "run_events_limit");
        assert_eq!(events.event_count, 3);
        assert_eq!(events.shown_count, 2);
        assert_eq!(events.events[0].kind, "tool_call_finished");
        assert_eq!(events.events[1].kind, "run_finished");
        assert!(state.transcript.iter().any(|item| {
            item.role == TranscriptRole::User && item.content == "/events run_events_limit 2"
        }));
    }

    #[tokio::test]
    async fn proposals_command_lists_store_items_and_updates_side_panel() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        let first = create_test_proposal(
            &state.options.store_path,
            "run_prop_1",
            "echo_agent",
            "edit_file",
            "Update a file",
            ProposalStatus::PendingApproval,
        )
        .await;
        create_test_proposal(
            &state.options.store_path,
            "run_prop_2",
            "echo_agent",
            "send_email",
            "Send a customer email",
            ProposalStatus::Approved,
        )
        .await;

        execute_command(&mut state, "/proposals")
            .await
            .expect("proposals command succeeds");

        let proposals = state.latest_proposals.as_ref().expect("proposal summary");
        assert_eq!(proposals.total_count, 2);
        assert_eq!(proposals.pending_count, 1);
        assert_eq!(proposals.approved_count, 1);
        assert!(state.transcript.iter().any(|item| {
            item.content.contains("Proposals: 2 total, 1 pending")
                && item.content.contains(&first.proposal_id.0)
                && item.content.contains("[pending_approval] edit_file")
        }));
        let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
        assert!(rendered.contains("proposals"));
        assert!(rendered.contains("total 2 pend 1 ok 1 deny 0"));
    }

    #[tokio::test]
    async fn proposal_command_defaults_to_single_loaded_proposal() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        let proposal = create_test_proposal(
            &state.options.store_path,
            "run_prop_default",
            "echo_agent",
            "edit_file",
            "Update one file",
            ProposalStatus::PendingApproval,
        )
        .await;

        execute_command(&mut state, "/proposals run_prop_default")
            .await
            .expect("proposals command succeeds");
        execute_command(&mut state, "/proposal")
            .await
            .expect("proposal command succeeds");

        assert!(state.transcript.iter().any(|item| {
            item.role == TranscriptRole::User
                && item.content == format!("/proposal {}", proposal.proposal_id.0)
        }));
        assert!(state.transcript.iter().any(|item| {
            item.content.contains(&proposal.proposal_id.0)
                && item.content.contains("\"summary\": \"Update one file\"")
        }));
        assert_eq!(
            state
                .latest_proposals
                .as_ref()
                .expect("proposal summary")
                .total_count,
            1
        );
    }

    #[tokio::test]
    async fn proposal_command_requires_id_when_multiple_proposals_are_loaded() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        create_test_proposal(
            &state.options.store_path,
            "run_prop_many_1",
            "echo_agent",
            "edit_file",
            "Update one file",
            ProposalStatus::PendingApproval,
        )
        .await;
        create_test_proposal(
            &state.options.store_path,
            "run_prop_many_2",
            "echo_agent",
            "send_email",
            "Send one email",
            ProposalStatus::PendingApproval,
        )
        .await;
        execute_command(&mut state, "/proposals")
            .await
            .expect("proposals command succeeds");

        let error = execute_command(&mut state, "/proposal")
            .await
            .expect_err("multiple proposals require explicit id");

        assert!(
            error
                .to_string()
                .contains("2 proposals are currently shown")
        );
    }

    #[tokio::test]
    async fn approve_proposal_command_updates_store_and_summary() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        let proposal = create_test_proposal(
            &state.options.store_path,
            "run_prop_approve",
            "echo_agent",
            "edit_file",
            "Update a file",
            ProposalStatus::PendingApproval,
        )
        .await;

        execute_command(
            &mut state,
            &format!("/approve-proposal {} looks good", proposal.proposal_id.0),
        )
        .await
        .expect("approve proposal succeeds");

        let stored = load_proposal(&state.options.store_path, &proposal.proposal_id)
            .await
            .expect("proposal loads");
        assert_eq!(stored.status, ProposalStatus::Approved);
        assert_eq!(
            stored.approval_decisions[0].comment.as_deref(),
            Some("looks good")
        );
        assert_eq!(
            state
                .latest_proposals
                .as_ref()
                .expect("proposal summary")
                .proposals[0]
                .status,
            "approved"
        );
        assert!(state.transcript.iter().any(|item| {
            item.content
                .contains(&format!("Proposal {} approved", proposal.proposal_id.0))
        }));
    }

    #[tokio::test]
    async fn deny_proposal_command_updates_store_and_summary() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;
        let proposal = create_test_proposal(
            &state.options.store_path,
            "run_prop_deny",
            "echo_agent",
            "send_email",
            "Send a customer email",
            ProposalStatus::PendingApproval,
        )
        .await;

        execute_command(
            &mut state,
            &format!("/deny-proposal {} too risky", proposal.proposal_id.0),
        )
        .await
        .expect("deny proposal succeeds");

        let stored = load_proposal(&state.options.store_path, &proposal.proposal_id)
            .await
            .expect("proposal loads");
        assert_eq!(stored.status, ProposalStatus::Denied);
        assert_eq!(
            state
                .latest_proposals
                .as_ref()
                .expect("proposal summary")
                .proposals[0]
                .status,
            "denied"
        );
    }

    #[tokio::test]
    async fn agents_command_lists_agents_and_use_switches_chat_target() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = multi_agent_state(&dir).await;

        assert_eq!(state.selected_agent_id.as_deref(), Some("echo_agent"));

        execute_command(&mut state, "/agents")
            .await
            .expect("agents command succeeds");

        assert!(state.transcript.iter().any(|item| {
            item.content.contains("Active agent: echo_agent")
                && item.content.contains("* echo_agent (Echo Agent)")
                && item.content.contains("  review_agent (Review Agent)")
        }));

        execute_command(&mut state, "/use review_agent")
            .await
            .expect("use command succeeds");

        assert_eq!(state.selected_agent_id.as_deref(), Some("review_agent"));
        assert!(state.status.contains("agent review_agent"));
        assert!(state.transcript.iter().any(|item| {
            item.content
                .contains("Using agent 'review_agent' for natural-language chat.")
        }));
        assert!(
            crate::tui::render::render_tui_once(&state)
                .expect("tui renders")
                .contains("agent: review_agent")
        );
    }

    #[tokio::test]
    async fn use_command_rejects_unknown_agent_without_switching() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = multi_agent_state(&dir).await;

        let error = execute_command(&mut state, "/use missing_agent")
            .await
            .expect_err("unknown agent is rejected");

        assert!(error.to_string().contains("unknown agent 'missing_agent'"));
        assert_eq!(state.selected_agent_id.as_deref(), Some("echo_agent"));
    }

    #[tokio::test]
    async fn tool_command_calls_active_services() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(&mut state, r#"/tool echo {"value":42}"#)
            .await
            .expect("tool command succeeds");

        assert!(
            state
                .events
                .iter()
                .any(|line| line == "tool policy: echo risk=read_only allowed=true")
        );
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Policy
                && activity.title == "tool policy"
                && activity.detail.as_deref() == Some("echo risk=read_only allowed=true")
        }));
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains(r#""value": 42"#))
        );
    }

    #[tokio::test]
    async fn tool_command_requests_approval_for_high_risk_tool() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = high_risk_echo_state(&dir, true).await;

        execute_command(
            &mut state,
            r#"/tool echo {"message":"from high-risk echo"}"#,
        )
        .await
        .expect("high-risk tool command requests approval");
        state.refresh_runs().await.expect("runs refresh");

        assert!(
            state
                .events
                .iter()
                .any(|line| line == "tool policy: echo risk=high allowed=true")
        );
        assert!(state.pending_approval.is_some());
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Approval && activity.title == "approval required"
        }));
        assert!(state.recent_runs.is_empty());
        let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
        assert!(rendered.contains("pending approval"));
        assert!(rendered.contains("echo (high)"));

        execute_command(&mut state, "/approve")
            .await
            .expect("approval executes high-risk tool");
        state.refresh_runs().await.expect("runs refresh");

        assert!(state.pending_approval.is_none());
        assert!(state.recent_runs.is_empty());
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Approval && activity.title == "approval granted"
        }));
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("from high-risk echo"))
        );
    }

    #[tokio::test]
    async fn tool_command_can_deny_high_risk_tool_call() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = high_risk_echo_state(&dir, true).await;

        execute_command(&mut state, r#"/tool echo {"message":"deny me"}"#)
            .await
            .expect("high-risk tool command requests approval");
        assert!(state.pending_approval.is_some());

        execute_command(&mut state, "/deny")
            .await
            .expect("deny succeeds");
        state.refresh_runs().await.expect("runs refresh");

        assert!(state.pending_approval.is_none());
        assert!(state.recent_runs.is_empty());
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Approval && activity.title == "approval denied"
        }));
    }

    #[tokio::test]
    async fn approval_aliases_accept_yes_and_no() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        state.set_pending_approval(TuiPendingApproval::tool_call(
            "echo",
            TuiToolRisk::High,
            json!({"value": "yes alias"}),
        ));
        execute_command(&mut state, "/yes")
            .await
            .expect("yes alias approves");

        assert!(state.pending_approval.is_none());
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Approval && activity.title == "approval granted"
        }));
        assert!(state.transcript.iter().any(|item| {
            item.role == TranscriptRole::Tool && item.content.contains("yes alias")
        }));

        state.set_pending_approval(TuiPendingApproval::tool_call(
            "echo",
            TuiToolRisk::High,
            json!({"value": "no alias"}),
        ));
        execute_command(&mut state, "/no")
            .await
            .expect("no alias denies");

        assert!(state.pending_approval.is_none());
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Approval && activity.title == "approval denied"
        }));
        assert!(
            !state
                .transcript
                .iter()
                .any(|item| item.content.contains("no alias"))
        );
    }

    #[tokio::test]
    async fn tool_command_blocks_high_risk_when_policy_denies_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = high_risk_echo_state(&dir, false).await;

        let error = execute_command(&mut state, r#"/tool echo {"message":"blocked"}"#)
            .await
            .expect_err("high-risk tool should be blocked");
        state.refresh_runs().await.expect("runs refresh");

        assert!(
            error
                .to_string()
                .contains("blocked by the current TUI tool policy")
        );
        assert!(
            state
                .events
                .iter()
                .any(|line| line == "tool policy: echo risk=high allowed=false")
        );
        assert!(state.recent_runs.is_empty());
    }

    async fn create_test_proposal(
        store_path: &Utf8PathBuf,
        run_id: &str,
        agent_id: &str,
        kind: &str,
        summary: &str,
        status: ProposalStatus,
    ) -> ProposalEnvelope {
        let store = FileProposalStore::new(store_path.clone())
            .await
            .expect("proposal store opens");
        let mut proposal = ProposalEnvelope::new(
            RunId(run_id.to_owned()),
            agent_id.to_owned(),
            kind.to_owned(),
            summary.to_owned(),
            json!({}),
        );
        proposal.status = status;
        store
            .create_proposal(proposal.clone())
            .await
            .expect("proposal writes");
        proposal
    }

    async fn create_test_run(
        store_path: &Utf8PathBuf,
        run_id: RunId,
        status: AgentRunStatus,
    ) -> AgentRunRecord {
        let store = FileRunStore::new(store_path.clone())
            .await
            .expect("run store opens");
        let now = OffsetDateTime::now_utc();
        let record = AgentRunRecord {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id,
            idempotency_key: None,
            agent_id: "echo_agent".to_owned(),
            status: status.clone(),
            scope: RunScope::Global,
            started_at: now,
            finished_at: (status != AgentRunStatus::Running).then_some(now),
            input: json!({}),
            output: json!({}),
            error: None,
            workflow: None,
            metadata: json!({}),
        };
        store.create_run(record.clone()).await.expect("run writes");
        record
    }
}
