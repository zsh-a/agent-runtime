use super::*;

#[tokio::test]
async fn approval_picker_keys_switch_selected_action() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.set_pending_approval(TuiPendingApproval::tool_call(
        "echo",
        TuiToolRisk::High,
        json!({}),
    ));
    let (sender, _receiver) = unbounded_channel();
    let mut active_task = None;

    assert_eq!(state.approval_selection, None);

    handle_input_key(
        &mut state,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        &sender,
        &mut active_task,
    )
    .await
    .expect("tab handled");
    assert_eq!(state.approval_selection, Some(TuiApprovalSelection::Deny));

    handle_input_key(
        &mut state,
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        &sender,
        &mut active_task,
    )
    .await
    .expect("left handled");
    assert_eq!(
        state.approval_selection,
        Some(TuiApprovalSelection::Approve)
    );

    handle_input_key(
        &mut state,
        KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        &sender,
        &mut active_task,
    )
    .await
    .expect("right handled");
    assert_eq!(state.approval_selection, Some(TuiApprovalSelection::Deny));
}

#[tokio::test]
async fn approval_picker_enter_confirms_selected_action() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.set_pending_approval(TuiPendingApproval::tool_call(
        "echo",
        TuiToolRisk::High,
        json!({"value": "skip"}),
    ));
    state.select_denial();
    let (sender, mut receiver) = unbounded_channel();
    let mut active_task = None;

    handle_input_key(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &sender,
        &mut active_task,
    )
    .await
    .expect("enter handled");

    assert!(state.pending_approval.is_none());
    assert!(state.busy);
    assert!(active_task.is_some());
    assert_eq!(state.approval_selection, None);
    assert!(state.activity.iter().any(|activity| {
        activity.kind == TuiActivityKind::Approval && activity.title == "approval denied"
    }));
    assert!(state.transcript.iter().any(|item| item.content == "no"));

    apply_updates_until_idle(&mut state, &mut receiver).await;
    if let Some(task) = active_task.take() {
        task.join.await.expect("approval task joins");
    }

    assert!(
        !state
            .transcript
            .iter()
            .any(|item| item.content.contains("skip"))
    );
    assert!(
        state
            .transcript
            .iter()
            .any(|item| { item.content.contains("Denied high-risk tool 'echo'.") })
    );
}

#[tokio::test]
async fn approval_requires_an_explicit_selection() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.set_pending_approval(TuiPendingApproval::tool_call(
        "echo",
        TuiToolRisk::High,
        json!({"value": "typed"}),
    ));
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

    assert!(!state.busy);
    assert!(active_task.is_none());
    assert!(state.pending_approval.is_some());
    assert_eq!(state.approval_selection, None);
}
