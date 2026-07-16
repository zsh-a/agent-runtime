use super::*;

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
            .contains("agent  review_agent")
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
