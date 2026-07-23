use super::*;
use agent_chat::{ChatToolExecution, ChatTurnRequest, chat_turn_initial_state};
use agent_core::PROTOCOL_VERSION;
use agent_llm::user_message;
use serde_json::json;

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
            context_policy: ContextPolicy::default(),
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
        pane_sizing: TuiPaneSizing::default(),
        text_selection: None,
        input_history: VecDeque::new(),
        history_cursor: None,
        history_draft: None,
        busy: false,
        operation_label: None,
        operation_started_at: None,
    }
}

fn test_chat_state() -> ChatTurnState {
    chat_turn_initial_state(&ChatTurnRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        turn_id: None,
        surface: Some("agent_tui".to_owned()),
        mode: Some("natural_language".to_owned()),
        session_id: None,
        thread_id: None,
        agent_id: Some("echo_agent".to_owned()),
        provider: "mock".to_owned(),
        model: "mock-model".to_owned(),
        messages: vec![user_message("hello")],
        temperature: None,
        max_output_tokens: None,
        tools: Vec::new(),
        context_blocks: Vec::new(),
        metadata: json!({}),
        context_policy: Default::default(),
        max_tool_rounds: 4,
        tool_execution: ChatToolExecution::Client,
    })
    .expect("chat state")
}

#[test]
fn initial_agent_prefers_configured_default() {
    let agents = vec![
        TuiAgentSummary {
            id: "echo_agent".to_owned(),
            name: "Echo Agent".to_owned(),
        },
        TuiAgentSummary {
            id: "review_agent".to_owned(),
            name: "Review Agent".to_owned(),
        },
    ];

    let selected =
        select_initial_agent(Some("review_agent"), &agents).expect("default agent resolves");

    assert_eq!(selected.as_deref(), Some("review_agent"));
}

#[test]
fn initial_agent_rejects_unknown_configured_default() {
    let agents = vec![TuiAgentSummary {
        id: "echo_agent".to_owned(),
        name: "Echo Agent".to_owned(),
    }];

    let error =
        select_initial_agent(Some("missing_agent"), &agents).expect_err("unknown is rejected");

    assert!(
        error
            .to_string()
            .contains("configured default agent 'missing_agent' was not found")
    );
}

#[test]
fn command_input_edits_at_cursor() {
    let mut state = test_state();
    state.replace_command_input("hello");

    state.move_cursor_left();
    state.move_cursor_left();
    state.insert_char('X');
    assert_eq!(state.command_input, "helXlo");

    state.backspace();
    assert_eq!(state.command_input, "hello");

    state.move_cursor_to_start();
    state.delete();
    assert_eq!(state.command_input, "ello");
    assert_eq!(state.input_cursor, 0);
}

#[test]
fn panel_focus_cycles_and_remembers_sidebar() {
    let mut state = test_state();

    state.focus_next_panel();
    assert_eq!(state.focused_panel, TuiFocusPanel::Context);
    assert_eq!(state.sidebar_panel, TuiFocusPanel::Context);
    assert!(!state.input_mode);

    state.focus_next_panel();
    assert_eq!(state.focused_panel, TuiFocusPanel::Activity);
    assert_eq!(state.sidebar_panel, TuiFocusPanel::Activity);

    state.focus_next_panel();
    assert_eq!(state.focused_panel, TuiFocusPanel::Chat);
    assert_eq!(state.sidebar_panel, TuiFocusPanel::Activity);

    state.focus_previous_panel();
    assert_eq!(state.focused_panel, TuiFocusPanel::Activity);
}

#[test]
fn editing_input_closes_completion_menu() {
    let mut state = test_state();
    state.show_completions(
        "Commands",
        vec![TuiCompletionItem {
            label: "/status".to_owned(),
            description: None,
            replacement: "/status".to_owned(),
        }],
    );

    state.insert_char('/');

    assert!(state.completion.is_none());
}

#[test]
fn operation_status_tracks_stage_and_completion() {
    let mut state = test_state();

    state.start_operation("thinking");
    assert!(state.busy);
    assert!(state.operation_status().starts_with("thinking "));

    state.set_operation_label("cancelling");
    assert!(state.operation_status().starts_with("cancelling "));

    state.set_busy(false);
    assert!(!state.busy);
    assert!(state.operation_label.is_none());
    assert!(state.operation_started_at.is_none());
}

#[test]
fn command_history_restores_unsubmitted_draft() {
    let mut state = test_state();
    state.remember_input("first");
    state.remember_input("second");
    state.replace_command_input("draft");

    state.history_previous();
    assert_eq!(state.command_input, "second");
    state.history_previous();
    assert_eq!(state.command_input, "first");
    state.history_next();
    assert_eq!(state.command_input, "second");
    state.history_next();
    assert_eq!(state.command_input, "draft");
    assert_eq!(state.history_cursor, None);
}

#[test]
fn assistant_replace_updates_stream_after_done() {
    let mut state = test_state();
    state.start_assistant_stream();
    state.apply_update(TuiUpdate::AssistantDelta("partial".to_owned()));
    state.apply_update(TuiUpdate::AssistantFinish);
    state.apply_update(TuiUpdate::AssistantReplace("final answer".to_owned()));

    let assistant_items = state
        .transcript
        .iter()
        .filter(|item| item.role == TranscriptRole::Assistant)
        .collect::<Vec<_>>();
    assert_eq!(assistant_items.len(), 1);
    assert_eq!(assistant_items[0].content, "final answer");
    assert!(!assistant_items[0].streaming);
}

#[test]
fn assistant_replace_after_tool_result_keeps_final_answer_visible() {
    let mut state = test_state();
    state.start_assistant_stream();
    state.apply_update(TuiUpdate::AssistantDelta("checking".to_owned()));
    state.apply_update(TuiUpdate::ToolMessage {
        title: Some("echo".to_owned()),
        content: "tool result".to_owned(),
    });
    state.apply_update(TuiUpdate::AssistantReplace("final answer".to_owned()));

    let assistant_items = state
        .transcript
        .iter()
        .filter(|item| item.role == TranscriptRole::Assistant)
        .collect::<Vec<_>>();
    assert_eq!(assistant_items.len(), 2);
    assert_eq!(assistant_items[0].content, "checking");
    assert_eq!(assistant_items[1].content, "final answer");
}

#[test]
fn pending_approval_summary_names_slash_and_chat_tools() {
    let slash = TuiPendingApproval::tool_call("shell.exec", TuiToolRisk::High, json!({}));
    assert_eq!(slash.subject(), "shell.exec");
    assert_eq!(slash.summary(), "shell.exec (high)");

    let chat = TuiPendingApproval::chat_tools(
        "echo_agent",
        TuiToolRisk::High,
        test_chat_state(),
        vec![
            ChatToolCall {
                id: "call_1".to_owned(),
                name: "shell.exec".to_owned(),
                input: json!({}),
            },
            ChatToolCall {
                id: "call_2".to_owned(),
                name: "echo".to_owned(),
                input: json!({}),
            },
        ],
        vec![user_message("hello")],
    );
    assert_eq!(chat.subject(), "shell.exec +1 tool(s)");
    assert_eq!(chat.summary(), "shell.exec +1 tool(s) (high)");
}

#[test]
fn pending_approval_update_sets_and_clears_state() {
    let mut state = test_state();
    let approval = TuiPendingApproval::tool_call("shell.exec", TuiToolRisk::High, json!({}));

    state.apply_update(TuiUpdate::PendingApproval(Some(approval)));
    assert_eq!(
        state
            .pending_approval
            .as_ref()
            .map(TuiPendingApproval::summary),
        Some("shell.exec (high)".to_owned())
    );

    state.apply_update(TuiUpdate::PendingApproval(None));
    assert!(state.pending_approval.is_none());
}
