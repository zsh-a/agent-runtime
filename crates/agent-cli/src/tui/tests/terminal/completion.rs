use super::*;

#[tokio::test]
async fn tab_completes_recent_run_id_for_run_commands() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.recent_runs = vec![
        test_run("run_alpha", AgentRunStatus::Completed),
        test_run("run_beta", AgentRunStatus::Running),
    ];
    state.replace_command_input("/events run_a");

    complete_slash_command(&mut state);

    assert_eq!(state.command_input, "/events run_alpha");
    assert_eq!(state.input_cursor, state.command_input.len());
}

#[tokio::test]
async fn tab_completes_agent_id_for_run_and_use_commands() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.agents = vec![TuiAgentSummary {
        id: "review_agent".to_owned(),
        name: "Review Agent".to_owned(),
    }];
    state.replace_command_input("/run review");

    complete_slash_command(&mut state);

    assert_eq!(state.command_input, "/run review_agent ");

    state.replace_command_input("/use review");
    complete_slash_command(&mut state);

    assert_eq!(state.command_input, "/use review_agent");
}

#[tokio::test]
async fn tab_completes_tool_name_for_tool_commands() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.replace_command_input("/tool ech");

    complete_slash_command(&mut state);

    assert_eq!(state.command_input, "/tool echo ");
}

#[tokio::test]
async fn tab_completes_help_topics() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.replace_command_input("/help ev");

    complete_slash_command(&mut state);

    assert_eq!(state.command_input, "/help events");

    state.replace_command_input("/? too");
    complete_slash_command(&mut state);

    assert_eq!(state.command_input, "/? tool");
}

#[tokio::test]
async fn tab_opens_help_topic_candidates_when_ambiguous() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.replace_command_input("/help pro");

    complete_slash_command(&mut state);

    assert_eq!(state.command_input, "/help pro");
    let menu = state.completion.as_ref().expect("completion menu opens");
    assert_eq!(menu.title, "Help topics");
    assert!(menu.items.iter().any(|item| item.label == "proposals"));
    assert!(menu.items.iter().any(|item| item.label == "proposal"));
}

#[tokio::test]
async fn tab_opens_run_id_candidates_when_ambiguous() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.recent_runs = vec![
        test_run("run_alpha", AgentRunStatus::Completed),
        test_run("run_beta", AgentRunStatus::Running),
    ];
    state.replace_command_input("/cancel run_");

    complete_slash_command(&mut state);

    assert_eq!(state.command_input, "/cancel run_");
    let menu = state.completion.as_ref().expect("completion menu opens");
    assert_eq!(menu.title, "Run ids");
    assert_eq!(menu.items[0].replacement, "/cancel run_alpha");
    assert_eq!(menu.items[1].replacement, "/cancel run_beta");
}

#[tokio::test]
async fn completion_menu_cycles_and_accepts_selection() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.replace_command_input("/help pro");
    complete_slash_command(&mut state);
    complete_slash_command(&mut state);
    let (sender, _receiver) = unbounded_channel();
    let mut active_task = None;

    handle_input_key(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &sender,
        &mut active_task,
    )
    .await
    .expect("enter handled");

    assert_eq!(state.command_input, "/help proposal");
    assert!(state.completion.is_none());
    assert!(!state.busy);
}

#[tokio::test]
async fn tab_completes_latest_proposal_id_for_proposal_commands() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.latest_proposals = Some(TuiProposalListSummary {
        total_count: 1,
        pending_count: 1,
        approved_count: 0,
        denied_count: 0,
        proposals: vec![TuiProposalSummary {
            proposal_id: "proposal_alpha".to_owned(),
            run_id: "run_alpha".to_owned(),
            agent_id: "echo_agent".to_owned(),
            kind: "edit_file".to_owned(),
            summary: "Update a file".to_owned(),
            status: "pending_approval".to_owned(),
            risk: "High".to_owned(),
            diff_count: 1,
            warning_count: 0,
        }],
    });
    state.replace_command_input("/approve-proposal proposal_a");

    complete_slash_command(&mut state);

    assert_eq!(state.command_input, "/approve-proposal proposal_alpha");
}

#[tokio::test]
async fn tab_completes_command_names_before_argument_mode() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.replace_command_input("/eve");

    complete_slash_command(&mut state);

    assert_eq!(state.command_input, "/events ");
}

#[tokio::test]
async fn tab_completes_status_command_name() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.replace_command_input("/sta");

    complete_slash_command(&mut state);

    assert_eq!(state.command_input, "/status");
}

#[tokio::test]
async fn ctrl_p_opens_global_command_palette() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.replace_command_input("draft");
    let (sender, _receiver) = unbounded_channel();
    let mut active_task = None;

    handle_input_key(
        &mut state,
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        &sender,
        &mut active_task,
    )
    .await
    .expect("ctrl-p handled");

    assert_eq!(state.command_input, "/");
    let menu = state.completion.as_ref().expect("command palette opens");
    assert_eq!(menu.title, "Commands");
    assert!(menu.items.iter().any(|item| item.label == "/run"));
    assert!(menu.items.iter().any(|item| item.label == "/tools"));

    handle_input_key(
        &mut state,
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        &sender,
        &mut active_task,
    )
    .await
    .expect("filter handled");

    assert_eq!(state.command_input, "/r");
    let menu = state
        .completion
        .as_ref()
        .expect("filtered palette stays open");
    assert!(menu.items.iter().all(|item| item.label.starts_with("/r")));
    assert!(!menu.items.iter().any(|item| item.label == "/tools"));
}
