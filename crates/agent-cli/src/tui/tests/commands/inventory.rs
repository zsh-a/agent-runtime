use super::*;

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
    assert!(rendered.contains("tools  1 | high 0 | blocked 0"));
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
