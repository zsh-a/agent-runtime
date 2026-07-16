use super::*;
use crate::{
    chat::ChatLlmOptions,
    config::RuntimeStoreBackend,
    runtime_config::ResolvedRuntimeSources,
    tools::ToolOverrides,
    tui::{
        data::{TuiAgentSummary, TuiOptions, TuiPendingApproval, TuiSelectionPoint},
        policy::TuiToolRisk,
    },
};
use camino::Utf8PathBuf;
use std::{collections::VecDeque, vec};

fn test_state() -> TuiState {
    TuiState {
        options: TuiOptions {
            runtime_sources: ResolvedRuntimeSources::new(Utf8PathBuf::from("agents.yaml"), None),
            trace_path: None,
            store_path: Utf8PathBuf::from("store"),
            store_backend: RuntimeStoreBackend::File,
            tool_overrides: ToolOverrides::default(),
            allow_high_risk_tools: true,
            chat: ChatLlmOptions {
                provider: "mock".to_owned(),
                model: "mock-model".to_owned(),
                mock_response: "ok".to_owned(),
                api_base_url: None,
                api_key_env: "OPENAI_API_KEY".to_owned(),
                anthropic_version: "2023-06-01".to_owned(),
                temperature: None,
                max_output_tokens: None,
                max_tool_rounds: 4,
            },
            timeout_seconds: 60,
            max_retries: 0,
            retry_backoff_ms: 0,
            hooks: Vec::new(),
            context_policy: Default::default(),
            default_agent: None,
            mouse_capture: false,
            once: false,
        },
        selected_agent_id: Some("echo_agent".to_owned()),
        agents: vec![TuiAgentSummary {
            id: "echo_agent".to_owned(),
            name: "Echo Agent".to_owned(),
        }],
        catalog_summary: None,
        trace: None,
        trace_label: None,
        recent_runs: Vec::new(),
        status: "ready".to_owned(),
        input_mode: true,
        command_input: String::new(),
        input_cursor: 0,
        completion: None,
        transcript: Vec::new(),
        active_assistant_index: None,
        events: VecDeque::new(),
        activity: VecDeque::new(),
        tool_inventory: None,
        context_status: None,
        latest_run: None,
        latest_workflow: None,
        latest_proposals: None,
        latest_events: None,
        pending_approval: None,
        approval_selection: None,
        chat_messages: Vec::new(),
        chat_scroll: 0,
        context_scroll: 0,
        event_scroll: 0,
        focused_panel: TuiFocusPanel::Chat,
        sidebar_panel: TuiFocusPanel::Context,
        detail_kind: TuiDetailKind::Overview,
        pane_sizing: Default::default(),
        text_selection: None,
        input_history: VecDeque::new(),
        history_cursor: None,
        history_draft: None,
        busy: false,
        operation_label: None,
        operation_started_at: None,
    }
}

#[test]
fn input_lines_place_cursor_after_inserted_text() {
    let mut state = test_state();
    state.replace_command_input("hello");
    state.move_cursor_left();
    let input = input_lines(&state, 24);

    assert_eq!(input.cursor_y, 0);
    assert_eq!(input.cursor_x, 6);
}

#[test]
fn input_lines_show_guided_placeholder() {
    let mut state = test_state();
    let input = input_lines(&state, 40);

    assert_eq!(input.lines[0].to_string(), "> message or /command");

    state.replace_command_input("/");
    let slash_input = input_lines(&state, 40);

    assert_eq!(slash_input.lines[0].to_string(), "/ command");
}

#[test]
fn input_cursor_for_click_maps_plain_text_position() {
    let mut state = test_state();
    state.replace_command_input("hello");
    let area = Rect::new(0, 0, 40, 3);

    let cursor = input_cursor_for_click(&state, area, 1 + 4, 1).expect("click maps to cursor");

    assert_eq!(cursor, 3);
}

#[test]
fn input_cursor_for_click_accounts_for_slash_prompt() {
    let mut state = test_state();
    state.replace_command_input("/status");
    let area = Rect::new(0, 0, 40, 3);

    let cursor = input_cursor_for_click(&state, area, 1 + 4, 1).expect("click maps to cursor");

    assert_eq!(cursor, 4);
}

#[test]
fn command_panel_uses_message_title() {
    let state = test_state();
    let rendered = render_tui_once(&state).expect("tui renders");

    assert!(rendered.contains("Message"));
}

#[test]
fn focused_panel_title_is_marked() {
    let mut state = test_state();
    state.leave_input_mode();
    let rendered = render_tui_once(&state).expect("tui renders");

    assert!(rendered.contains("> Chat"));

    state.focus_panel(TuiFocusPanel::Activity);
    let rendered = render_tui_once(&state).expect("tui renders");

    assert!(rendered.contains("> Details  [Timeline]"));
}

#[test]
fn selected_text_uses_panel_local_columns() {
    let mut state = test_state();
    state.push_user_message("alpha");
    state.begin_text_selection(TuiFocusPanel::Chat, TuiSelectionPoint::new(2, 1));
    state.update_text_selection(TuiFocusPanel::Chat, TuiSelectionPoint::new(7, 1));
    let area = Rect::new(0, 0, 80, 24);
    let layout = TuiLayout::new(
        area,
        state.pane_sizing,
        input_panel_height(&state, area),
        state.focused_panel,
        state.sidebar_panel,
    );

    let selected = selected_text_for_layout(&state, layout).expect("selection has text");

    assert_eq!(selected, "alpha");
}

#[test]
fn input_panel_prioritizes_pending_approval_guidance() {
    let mut state = test_state();
    state.pending_approval = Some(TuiPendingApproval::tool_call(
        "shell.exec",
        TuiToolRisk::High,
        serde_json::json!({}),
    ));

    let rendered = render_tui_once(&state).expect("tui renders");

    assert!(rendered.contains("Approval required"));
    assert!(rendered.contains("shell.exec"));
    assert!(rendered.contains(" Approve "));
    assert!(rendered.contains(" Deny "));
}

#[test]
fn approval_picker_renders_selected_deny_option() {
    let mut state = test_state();
    state.pending_approval = Some(TuiPendingApproval::tool_call(
        "shell.exec",
        TuiToolRisk::High,
        serde_json::json!({}),
    ));
    state.approval_selection = Some(TuiApprovalSelection::Deny);

    let rendered = render_tui_once(&state).expect("tui renders");

    assert!(rendered.contains("[Deny]"));
}

#[test]
fn approval_starts_without_a_selected_action() {
    let mut state = test_state();
    state.set_pending_approval(TuiPendingApproval::tool_call(
        "shell.exec",
        TuiToolRisk::High,
        serde_json::json!({"command": "rm -rf build"}),
    ));

    let rendered = render_tui_once(&state).expect("tui renders");

    assert!(rendered.contains("Approval required"));
    assert!(!rendered.contains("[Approve]"));
    assert!(!rendered.contains("[Deny]"));
}

#[test]
fn compact_layout_renders_one_workspace_panel() {
    let mut state = test_state();
    let chat = render_tui_at_size(&state, 72, 24).expect("compact chat renders");
    assert!(chat.contains("Chat"));
    assert!(!chat.contains("[Details]"));

    state.leave_input_mode();
    state.focus_panel(TuiFocusPanel::Context);
    let details = render_tui_at_size(&state, 72, 24).expect("compact details render");
    assert!(details.contains("[Details]"));
    assert!(!details.contains("No messages yet"));
}

#[test]
fn completion_menu_renders_above_input() {
    let mut state = test_state();
    state.replace_command_input("/help pro");
    state.show_completions(
        "Help topics",
        vec![
            crate::tui::data::TuiCompletionItem {
                label: "proposals".to_owned(),
                description: Some("Review proposals".to_owned()),
                replacement: "/help proposals".to_owned(),
            },
            crate::tui::data::TuiCompletionItem {
                label: "proposal".to_owned(),
                description: None,
                replacement: "/help proposal".to_owned(),
            },
        ],
    );

    let rendered = render_tui_once(&state).expect("tui renders");

    assert!(rendered.contains("Help topics"));
    assert!(rendered.contains("proposals"));
    assert!(rendered.contains("proposal"));
    assert!(rendered.contains("Review proposals"));
}

#[test]
fn status_line_shows_operation_stage_and_elapsed_time() {
    let mut state = test_state();
    state.start_operation("thinking");

    let rendered = render_tui_once(&state).expect("tui renders");

    assert!(rendered.contains("thinking "));
}

#[test]
fn long_chat_renders_scrollbar() {
    let mut state = test_state();
    for index in 0..16 {
        state.push_user_message(format!("message {index}"));
    }

    let rendered = render_tui_at_size(&state, 72, 18).expect("compact chat renders");

    assert!(rendered.contains('┃'));
}

#[test]
fn bottom_window_keeps_latest_items_by_default() {
    let items = vec![1, 2, 3, 4, 5];

    assert_eq!(bottom_window(items.clone(), 3, 0), vec![3, 4, 5]);
    assert_eq!(bottom_window(items, 3, 2), vec![1, 2, 3]);
}
