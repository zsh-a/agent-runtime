use super::*;

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
async fn run_command_uses_sqlite_store_backend() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut options = test_options(&dir, "mock response", true);
    options.store_backend = RuntimeStoreBackend::Sqlite;
    let mut state = TuiState::load(options).await.expect("state loads");

    execute_command(&mut state, "/run echo_agent sqlite tui")
        .await
        .expect("sqlite run command succeeds");

    let trace = state.trace.as_ref().expect("trace captured");
    let run_id = trace.run_id.clone();
    let stores = RuntimeStores::open(
        RuntimeStoreBackend::Sqlite,
        state.options.store_path.clone(),
    )
    .await
    .expect("sqlite stores open");
    assert!(
        stores
            .run_store
            .get_run(&run_id)
            .await
            .expect("run reads")
            .is_some()
    );
    assert!(
        stores
            .trace_store
            .read_trace(&run_id)
            .await
            .expect("trace reads")
            .is_some()
    );
    assert!(
        !state
            .options
            .store_path
            .join("runs")
            .join(format!("{}.json", run_id.0))
            .exists(),
        "sqlite TUI run should not write through the file run store"
    );
    assert!(
        !state
            .options
            .store_path
            .join("traces")
            .join(format!("{}.trace.json", run_id.0))
            .exists(),
        "sqlite TUI trace should not write through the file trace store"
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
        activity.kind == TuiActivityKind::Cancellation && activity.title == "cancellation requested"
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
