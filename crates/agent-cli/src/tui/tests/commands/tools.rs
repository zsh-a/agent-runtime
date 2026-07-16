use super::*;

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
    assert!(rendered.contains("Approval required"));
    assert!(rendered.contains("Tool  echo"));
    assert!(rendered.contains("Risk  high"));

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
    assert!(
        state.transcript.iter().any(|item| {
            item.role == TranscriptRole::Tool && item.content.contains("yes alias")
        })
    );

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
