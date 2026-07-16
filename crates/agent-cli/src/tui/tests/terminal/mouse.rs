use super::*;

#[tokio::test]
async fn mouse_wheel_scrolls_panel_under_pointer() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    let (sender, _receiver) = unbounded_channel();
    let mut active_task = None;

    handle_mouse_event(
        &mut state,
        mouse_event(MouseEventKind::ScrollUp, 10, 5),
        100,
        30,
        &sender,
        &mut active_task,
    );
    assert_eq!(state.chat_scroll, 4);
    assert_eq!(state.focused_panel, TuiFocusPanel::Chat);

    handle_mouse_event(
        &mut state,
        mouse_event(MouseEventKind::ScrollDown, 10, 5),
        100,
        30,
        &sender,
        &mut active_task,
    );
    assert_eq!(state.chat_scroll, 0);

    handle_mouse_event(
        &mut state,
        mouse_event(MouseEventKind::ScrollDown, 80, 2),
        100,
        30,
        &sender,
        &mut active_task,
    );
    assert_eq!(state.context_scroll, 4);
    assert_eq!(state.focused_panel, TuiFocusPanel::Context);

    handle_mouse_event(
        &mut state,
        mouse_event(MouseEventKind::ScrollUp, 80, 2),
        100,
        30,
        &sender,
        &mut active_task,
    );
    assert_eq!(state.context_scroll, 0);

    state.focus_panel(TuiFocusPanel::Activity);
    handle_mouse_event(
        &mut state,
        mouse_event(MouseEventKind::ScrollUp, 80, 20),
        100,
        30,
        &sender,
        &mut active_task,
    );
    assert_eq!(state.event_scroll, 4);
    assert_eq!(state.focused_panel, TuiFocusPanel::Activity);
}

#[tokio::test]
async fn mouse_click_selects_focused_panel_for_keyboard_scroll() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    let (sender, _receiver) = unbounded_channel();
    let mut active_task = None;

    handle_mouse_event(
        &mut state,
        mouse_event(MouseEventKind::Down(MouseButton::Left), 80, 2),
        100,
        30,
        &sender,
        &mut active_task,
    );
    assert_eq!(state.focused_panel, TuiFocusPanel::Context);

    handle_input_key(
        &mut state,
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        &sender,
        &mut active_task,
    )
    .await
    .expect("page down handled");

    assert_eq!(state.context_scroll, 4);
    assert_eq!(state.chat_scroll, 0);
    assert_eq!(state.event_scroll, 0);

    let layout = mouse_layout(&state, 100, 30);
    handle_mouse_event(
        &mut state,
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            layout.context.x + 13,
            layout.context.y,
        ),
        100,
        30,
        &sender,
        &mut active_task,
    );
    assert_eq!(state.focused_panel, TuiFocusPanel::Activity);

    handle_input_key(
        &mut state,
        KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
        &sender,
        &mut active_task,
    )
    .await
    .expect("page up handled");

    assert_eq!(state.event_scroll, 4);
}

#[tokio::test]
async fn mouse_drag_resizes_tui_panes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    let (sender, _receiver) = unbounded_channel();
    let mut active_task = None;
    let mut mouse_drag = None;

    let layout = mouse_layout(&state, 100, 30);
    handle_mouse_event_with_drag(
        &mut state,
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            layout.context.x,
            layout.chat.y + 5,
        ),
        100,
        30,
        &sender,
        &mut active_task,
        &mut mouse_drag,
    );
    handle_mouse_event_with_drag(
        &mut state,
        mouse_event(
            MouseEventKind::Drag(MouseButton::Left),
            62,
            layout.chat.y + 5,
        ),
        100,
        30,
        &sender,
        &mut active_task,
        &mut mouse_drag,
    );

    let resized = mouse_layout(&state, 100, 30);
    assert_eq!(resized.context.x, 62);
    assert_eq!(state.pane_sizing.side_width, Some(38));
}

#[tokio::test]
async fn mouse_clicking_recent_run_prefills_inspect_command() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.recent_runs = vec![
        test_run("run_alpha", AgentRunStatus::Completed),
        test_run("run_beta", AgentRunStatus::Running),
    ];
    state.focus_panel(TuiFocusPanel::Activity);
    let layout = mouse_layout(&state, 100, 30);
    let activity_count = 1usize;
    let full_len = activity_count + 2 + state.recent_runs.len();
    let height = layout.activity.height.saturating_sub(2) as usize;
    let start = full_len.saturating_sub(height);
    let first_run_full_index = activity_count + 2;
    let row = layout.activity.y + 1 + (first_run_full_index - start) as u16;
    let (sender, _receiver) = unbounded_channel();
    let mut active_task = None;

    handle_mouse_event(
        &mut state,
        mouse_event(MouseEventKind::Down(MouseButton::Left), 80, row),
        100,
        30,
        &sender,
        &mut active_task,
    );

    assert_eq!(state.focused_panel, TuiFocusPanel::Activity);
    assert_eq!(state.command_input, "/inspect run_alpha");
    assert_eq!(state.input_cursor, state.command_input.len());
    assert!(state.activity.iter().any(|activity| {
        activity.kind == TuiActivityKind::System
            && activity.title == "run selected"
            && activity.detail.as_deref() == Some("run_alpha")
    }));
}

#[tokio::test]
async fn mouse_clicking_context_proposal_prefills_proposal_command() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.set_latest_proposals(TuiProposalListSummary {
        total_count: 2,
        pending_count: 1,
        approved_count: 1,
        denied_count: 0,
        proposals: vec![
            TuiProposalSummary {
                proposal_id: "proposal_alpha".to_owned(),
                run_id: "run_alpha".to_owned(),
                agent_id: "echo_agent".to_owned(),
                kind: "edit_file".to_owned(),
                summary: "Update a file".to_owned(),
                status: "pending_approval".to_owned(),
                risk: "High".to_owned(),
                diff_count: 1,
                warning_count: 0,
            },
            TuiProposalSummary {
                proposal_id: "proposal_beta".to_owned(),
                run_id: "run_beta".to_owned(),
                agent_id: "echo_agent".to_owned(),
                kind: "write_file".to_owned(),
                summary: "Write a file".to_owned(),
                status: "approved".to_owned(),
                risk: "Medium".to_owned(),
                diff_count: 1,
                warning_count: 0,
            },
        ],
    });
    let layout = mouse_layout(&state, 100, 30);
    let proposal_column = layout.context.x + 2;
    let first_proposal_row = (layout.context.y + 1..layout.context.y + layout.context.height - 1)
        .find(|row| {
            context_action_for_click(&state, layout.context, proposal_column, *row)
                == Some(TuiContextClickAction::InspectProposal(
                    "proposal_alpha".to_owned(),
                ))
        })
        .expect("proposal row is visible");
    let (sender, _receiver) = unbounded_channel();
    let mut active_task = None;

    handle_mouse_event(
        &mut state,
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            proposal_column,
            first_proposal_row,
        ),
        100,
        30,
        &sender,
        &mut active_task,
    );

    assert_eq!(state.focused_panel, TuiFocusPanel::Context);
    assert_eq!(state.command_input, "/proposal proposal_alpha");
    assert_eq!(state.input_cursor, state.command_input.len());
    assert!(state.activity.iter().any(|activity| {
        activity.kind == TuiActivityKind::System
            && activity.title == "proposal selected"
            && activity.detail.as_deref() == Some("proposal_alpha")
    }));
}

#[tokio::test]
async fn mouse_wheel_over_input_browses_history_without_changing_panel_focus() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.remember_input("/status");
    state.remember_input("/runs");
    state.focus_panel(TuiFocusPanel::Activity);
    let layout = mouse_layout(&state, 100, 30);
    let input_column = layout.input.x + 4;
    let input_row = layout.input.y + 1;
    let (sender, _receiver) = unbounded_channel();
    let mut active_task = None;

    handle_mouse_event(
        &mut state,
        mouse_event(MouseEventKind::ScrollUp, input_column, input_row),
        100,
        30,
        &sender,
        &mut active_task,
    );
    assert_eq!(state.command_input, "/runs");
    assert_eq!(state.focused_panel, TuiFocusPanel::Activity);

    handle_mouse_event(
        &mut state,
        mouse_event(MouseEventKind::ScrollUp, input_column, input_row),
        100,
        30,
        &sender,
        &mut active_task,
    );
    assert_eq!(state.command_input, "/status");

    handle_mouse_event(
        &mut state,
        mouse_event(MouseEventKind::ScrollDown, input_column, input_row),
        100,
        30,
        &sender,
        &mut active_task,
    );
    assert_eq!(state.command_input, "/runs");
}

#[tokio::test]
async fn mouse_clicking_input_keeps_panel_focus_and_enters_input_mode() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.replace_command_input("hello");
    state.input_mode = false;
    state.focus_panel(TuiFocusPanel::Context);
    let layout = mouse_layout(&state, 100, 30);
    let (sender, _receiver) = unbounded_channel();
    let mut active_task = None;

    handle_mouse_event(
        &mut state,
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            layout.input.x + 5,
            layout.input.y + 1,
        ),
        100,
        30,
        &sender,
        &mut active_task,
    );

    assert!(state.input_mode);
    assert_eq!(state.input_cursor, 3);
    assert_eq!(state.focused_panel, TuiFocusPanel::Context);
}

#[tokio::test]
async fn mouse_clicking_approval_picker_starts_background_decision() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.set_pending_approval(TuiPendingApproval::tool_call(
        "echo",
        TuiToolRisk::High,
        json!({"value": "mouse"}),
    ));
    let area = Rect::new(0, 0, 100, 30);
    let modal = crate::tui::render::approval_modal_area(area);
    let deny_column = modal.x + modal.width / 2 + 4;
    let option_row = modal.y + modal.height - 3;
    let (sender, mut receiver) = unbounded_channel();
    let mut active_task = None;

    handle_mouse_event(
        &mut state,
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            deny_column,
            option_row,
        ),
        100,
        30,
        &sender,
        &mut active_task,
    );

    assert!(state.pending_approval.is_none());
    assert!(state.busy);
    assert!(active_task.is_some());
    assert!(state.transcript.iter().any(|item| item.content == "no"));

    apply_updates_until_idle(&mut state, &mut receiver).await;
    if let Some(task) = active_task.take() {
        task.join.await.expect("approval task joins");
    }

    assert!(
        state
            .transcript
            .iter()
            .any(|item| item.content.contains("Denied high-risk tool 'echo'."))
    );
}

#[tokio::test]
async fn mouse_clicking_completion_accepts_candidate() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    state.replace_command_input("/help pro");
    complete_slash_command(&mut state);
    let area = Rect::new(0, 0, 100, 30);
    let layout = mouse_layout(&state, area.width, area.height);
    let menu = crate::tui::render::completion_menu_area(&state, area, layout.input)
        .expect("completion menu area");
    let (sender, _receiver) = unbounded_channel();
    let mut active_task = None;

    handle_mouse_event(
        &mut state,
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 1,
        ),
        area.width,
        area.height,
        &sender,
        &mut active_task,
    );

    assert_eq!(state.command_input, "/help proposals");
    assert!(state.completion.is_none());
}
