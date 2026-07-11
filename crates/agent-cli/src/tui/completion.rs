use super::data::{TuiCompletionItem, TuiState};

struct CommandCompletion {
    text: &'static str,
    description: &'static str,
}

const COMMANDS: &[CommandCompletion] = &[
    command("help", "Command help"),
    command("status", "Runtime summary"),
    command("agents", "Available agents"),
    command("use ", "Switch active agent"),
    command("tools", "Tool inventory"),
    command("clear", "Clear this session"),
    command("refresh", "Reload runtime data"),
    command("run ", "Run an agent"),
    command("runs", "Recent runs"),
    command("cancel ", "Cancel a run"),
    command("events ", "Trace events"),
    command("workflow ", "Run workflow file"),
    command("proposals", "Review proposals"),
    command("proposal ", "Inspect proposal"),
    command("approve-proposal ", "Approve proposal"),
    command("deny-proposal ", "Deny proposal"),
    command("tool ", "Call a tool"),
    command("replay ", "Load a trace"),
    command("inspect ", "Inspect a run"),
];

const fn command(text: &'static str, description: &'static str) -> CommandCompletion {
    CommandCompletion { text, description }
}

pub(super) fn open_command_palette(state: &mut TuiState) {
    state.enter_command("/");
    show_command_matches(state, "", false, false);
}

pub(super) fn refresh_command_palette(state: &mut TuiState) {
    let input = state.command_input.clone();
    let Some(body) = input.strip_prefix('/') else {
        return;
    };
    if !body.contains(char::is_whitespace) {
        show_command_matches(state, body, false, false);
    }
}

pub(super) fn complete_slash_command(state: &mut TuiState) {
    if state.select_next_completion() || state.input_cursor != state.command_input.len() {
        return;
    }
    let input = state.command_input.clone();
    let Some(body) = input.strip_prefix('/') else {
        return;
    };
    if !body.contains(char::is_whitespace) {
        show_command_matches(state, body, true, true);
        return;
    }

    let (verb, rest) = body
        .split_once(char::is_whitespace)
        .map(|(verb, rest)| (verb.trim(), rest.trim_start()))
        .unwrap_or((body.trim(), ""));
    if rest.contains(char::is_whitespace) {
        return;
    }
    match verb {
        "help" | "?" => complete_argument(
            state,
            verb,
            rest,
            help_topics(),
            "",
            "help topic",
            "Help topics",
        ),
        "use" => complete_argument(
            state,
            verb,
            rest,
            agent_ids(state),
            "",
            "agent id",
            "Agent ids",
        ),
        "run" => complete_argument(
            state,
            verb,
            rest,
            agent_ids(state),
            " ",
            "agent id",
            "Agent ids",
        ),
        "tool" | "call" => complete_argument(
            state,
            verb,
            rest,
            tool_names(state),
            " ",
            "tool name",
            "Tool names",
        ),
        "inspect" | "events" | "cancel" | "proposals" => complete_argument(
            state,
            verb,
            rest,
            recent_run_ids(state),
            "",
            "run id",
            "Run ids",
        ),
        "proposal" | "approve-proposal" | "deny-proposal" => complete_argument(
            state,
            verb,
            rest,
            proposal_ids(state),
            "",
            "proposal id",
            "Proposal ids",
        ),
        _ => {}
    }
}

fn show_command_matches(
    state: &mut TuiState,
    typed: &str,
    complete_single: bool,
    report_empty: bool,
) {
    let matches = COMMANDS
        .iter()
        .filter(|command| command.text.starts_with(typed))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [command] if complete_single && !typed.is_empty() => {
            state.replace_command_input(format!("/{}", command.text));
        }
        [] if report_empty => state.push_event("no slash command matches"),
        [] => state.clear_completions(),
        commands => state.show_completions(
            "Commands",
            commands
                .iter()
                .map(|command| TuiCompletionItem {
                    label: format!("/{}", command.text.trim_end()),
                    description: Some(command.description.to_owned()),
                    replacement: format!("/{}", command.text),
                })
                .collect(),
        ),
    }
}

fn complete_argument(
    state: &mut TuiState,
    verb: &str,
    typed: &str,
    candidates: Vec<String>,
    suffix: &str,
    singular: &str,
    title: &str,
) {
    let matches = candidates
        .into_iter()
        .filter(|candidate| candidate.starts_with(typed))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [candidate] => state.replace_command_input(format!("/{verb} {candidate}{suffix}")),
        [] => state.push_event(format!("no {singular} matches")),
        candidates => state.show_completions(
            title,
            candidates
                .iter()
                .map(|candidate| TuiCompletionItem {
                    label: candidate.clone(),
                    description: None,
                    replacement: format!("/{verb} {candidate}{suffix}"),
                })
                .collect(),
        ),
    }
}

fn help_topics() -> Vec<String> {
    [
        "agents",
        "status",
        "use",
        "run",
        "runs",
        "inspect",
        "events",
        "workflow",
        "wf",
        "proposals",
        "proposal",
        "approve-proposal",
        "deny-proposal",
        "tool",
        "call",
        "cancel",
        "trace",
        "replay",
        "refresh",
        "clear",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}

fn agent_ids(state: &TuiState) -> Vec<String> {
    unique(state.agents.iter().map(|agent| agent.id.clone()))
}

fn tool_names(state: &TuiState) -> Vec<String> {
    state
        .tool_inventory
        .as_ref()
        .map(|inventory| unique(inventory.items.iter().map(|tool| tool.name.clone())))
        .unwrap_or_default()
}

fn recent_run_ids(state: &TuiState) -> Vec<String> {
    unique(state.recent_runs.iter().map(|run| run.run_id.0.clone()))
}

fn proposal_ids(state: &TuiState) -> Vec<String> {
    state
        .latest_proposals
        .as_ref()
        .map(|proposals| {
            unique(
                proposals
                    .proposals
                    .iter()
                    .map(|proposal| proposal.proposal_id.clone()),
            )
        })
        .unwrap_or_default()
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values.into_iter().fold(Vec::new(), |mut items, value| {
        if !items.contains(&value) {
            items.push(value);
        }
        items
    })
}
