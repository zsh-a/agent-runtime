use super::*;

pub(in crate::tui) async fn execute_command(state: &mut TuiState, input: &str) -> Result<()> {
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
            When the approval dialog is shown, select Approve or Deny, then confirm.\n\n\
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
