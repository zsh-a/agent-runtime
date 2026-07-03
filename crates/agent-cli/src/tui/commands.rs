use agent_core::{AgentRunStore, RunId};
use agent_store::FileRunStore;
use camino::Utf8PathBuf;
use miette::{IntoDiagnostic, Result, miette};
use serde_json::{Value, json};

use super::{
    approval::{approve_pending_tool, call_tool_or_request_approval, deny_pending_tool},
    chat::run_natural_language_command,
    data::{TuiActivityItem, TuiActivityKind, TuiState, read_trace},
    format::{compact_json, pretty_json},
    runtime::TuiRuntime,
    tool_inventory::{TuiToolInventory, load_tui_tool_inventory},
};

pub(super) async fn execute_command(state: &mut TuiState, input: &str) -> Result<()> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(());
    }
    let Some(command) = input.strip_prefix('/') else {
        return run_natural_language_command(state, input).await;
    };
    execute_slash_command(state, command.trim()).await
}

async fn execute_slash_command(state: &mut TuiState, input: &str) -> Result<()> {
    let (verb, rest) = split_once(input);
    match verb {
        "help" | "?" => show_help(state),
        "clear" => state.clear_output(),
        "refresh" => {
            state.refresh().await?;
            state.push_activity(TuiActivityItem::new(
                TuiActivityKind::System,
                "refreshed catalog/trace/store",
            ));
        }
        "tools" => show_tools_command(state).await?,
        "run" => run_agent_command(state, rest).await?,
        "tool" | "call" => tool_call_command(state, rest).await?,
        "approve" => approve_pending_tool(state).await?,
        "deny" => deny_pending_tool(state).await?,
        "trace" | "replay" => load_trace_command(state, rest).await?,
        "inspect" => inspect_run_command(state, rest).await?,
        other => state.push_system_message(format!(
            "unknown command '/{other}'. Try: /help, /tools, /run, /tool, /approve, /deny, /replay, /inspect, /refresh, /clear"
        )),
    }
    Ok(())
}

fn show_help(state: &mut TuiState) {
    state.push_system_message(
        "Type natural language and press Enter to chat with the default agent.\n\n\
        Slash commands:\n\
        /tools                      list chat tools, risks, sources, and policy status\n\
        /run <agent_id> [json|text]  run a specific runtime agent\n\
        /tool <name> [json]          call a tool through active CLI services\n\
        /approve                     approve the pending high-risk tool call\n\
        /deny                        deny the pending high-risk tool call\n\
        /replay <trace_path>         load a trace into the side panel\n\
        /inspect <run_id>            load a persisted run record summary\n\
        /refresh                     reload catalog, trace, and recent runs\n\
        /clear                       clear chat and activity\n\n\
        Keys:\n\
        Enter sends, Shift+Enter inserts a newline, Esc/Ctrl-C cancels a running task\n\
        Left/Right move the cursor, Ctrl/Alt+Left/Right move by word\n\
        Ctrl+A/E jump to start/end, Ctrl+U/K delete before/after cursor\n\
        Ctrl+W deletes the previous word, Up/Down browse input history\n\
        PageUp/PageDown scroll chat, Tab completes slash commands",
    );
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

async fn inspect_run_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let run_id = rest.trim();
    if run_id.is_empty() {
        return Err(miette!("run id is required"));
    }
    let store = FileRunStore::new(state.options.store_path.clone())
        .await
        .into_diagnostic()?;
    let record = store
        .get_run(&RunId(run_id.to_owned()))
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("run '{run_id}' was not found"))?;
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Run,
        format!("run {run_id}"),
        format!("{} {:?}", record.agent_id, record.status),
    ));
    state.push_system_message(pretty_json(&record));
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
        data::TranscriptRole,
        test_support::{test_state, test_state_with_policy},
    };

    #[tokio::test]
    async fn tools_command_lists_runtime_tools_and_policy_status() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(&mut state, "/tools")
            .await
            .expect("tools command succeeds");

        let inventory = state.tool_inventory.as_ref().expect("inventory is loaded");
        assert_eq!(inventory.total_count(), 2);
        assert_eq!(inventory.high_risk_count(), 1);
        assert_eq!(inventory.blocked_count(), 0);
        assert!(state.transcript.iter().any(|item| {
            item.content
                .contains("- agent.run [high / approval / agent_runtime_builtin]")
                && item
                    .content
                    .contains("- echo [read_only / allowed / agent_cli_builtin]")
        }));
        let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
        assert!(rendered.contains("tools 2 high 1 blocked 0"));
    }

    #[tokio::test]
    async fn tools_command_marks_high_risk_tools_blocked_by_policy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state_with_policy(&dir, "mock response", false).await;

        execute_command(&mut state, "/tools")
            .await
            .expect("tools command succeeds");

        let inventory = state.tool_inventory.as_ref().expect("inventory is loaded");
        assert_eq!(inventory.blocked_count(), 1);
        assert!(state.transcript.iter().any(|item| {
            item.content
                .contains("- agent.run [high / blocked / agent_runtime_builtin]")
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
    async fn tool_command_can_run_agent_run_tool() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(
            &mut state,
            r#"/tool agent.run {"agent_id":"echo_agent","input":{"message":"from agent.run"}}"#,
        )
        .await
        .expect("agent.run tool command requests approval");
        state.refresh_runs().await.expect("runs refresh");

        assert!(
            state
                .events
                .iter()
                .any(|line| line == "tool policy: agent.run risk=high allowed=true")
        );
        assert!(state.pending_approval.is_some());
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Approval && activity.title == "approval required"
        }));
        assert!(state.recent_runs.is_empty());
        let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
        assert!(rendered.contains("pending approval"));
        assert!(rendered.contains("agent.run (high)"));

        execute_command(&mut state, "/approve")
            .await
            .expect("approval executes agent.run");
        state.refresh_runs().await.expect("runs refresh");

        assert!(state.pending_approval.is_none());
        assert_eq!(state.recent_runs.len(), 1);
        assert_eq!(state.recent_runs[0].agent_id, "echo_agent");
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Approval && activity.title == "approval granted"
        }));
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("from agent.run"))
        );
    }

    #[tokio::test]
    async fn tool_command_can_deny_high_risk_tool_call() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "mock response").await;

        execute_command(
            &mut state,
            r#"/tool agent.run {"agent_id":"echo_agent","input":{"message":"deny me"}}"#,
        )
        .await
        .expect("agent.run tool command requests approval");
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
    async fn tool_command_blocks_high_risk_when_policy_denies_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state_with_policy(&dir, "mock response", false).await;

        let error = execute_command(
            &mut state,
            r#"/tool agent.run {"agent_id":"echo_agent","input":{"message":"blocked"}}"#,
        )
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
                .any(|line| line == "tool policy: agent.run risk=high allowed=false")
        );
        assert!(state.recent_runs.is_empty());
    }
}
