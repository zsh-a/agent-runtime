use super::*;

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
