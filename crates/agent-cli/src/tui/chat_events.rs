use agent_chat::{ChatTurnEvent, ChatTurnEventKind};
use serde_json::Value;

#[cfg(test)]
use super::data::TuiState;
use super::{
    data::{TuiActivityItem, TuiActivityKind, TuiContextStatus, TuiUpdate},
    format::compact_json,
};

#[cfg(test)]
pub(super) fn apply_chat_event_to_tui(
    state: &mut TuiState,
    event: &ChatTurnEvent,
    assistant_text: &mut String,
    final_response: &mut Option<agent_llm::LlmResponse>,
) {
    for update in updates_from_chat_event(event, assistant_text, final_response) {
        state.apply_update(update);
    }
}

pub(super) fn updates_from_chat_event(
    event: &ChatTurnEvent,
    assistant_text: &mut String,
    final_response: &mut Option<agent_llm::LlmResponse>,
) -> Vec<TuiUpdate> {
    let mut updates = Vec::new();
    match event.kind {
        ChatTurnEventKind::Started => {
            let provider = event
                .metadata
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let model = event
                .metadata
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                TuiActivityKind::Chat,
                "chat started",
                format!("{provider}/{model}"),
            )));
        }
        ChatTurnEventKind::LlmStarted => {
            updates.push(TuiUpdate::Activity(TuiActivityItem::new(
                TuiActivityKind::Chat,
                format!("round {} started", event.round),
            )));
        }
        ChatTurnEventKind::Delta => {
            if let Some(content) = &event.content {
                assistant_text.push_str(content);
                updates.push(TuiUpdate::AssistantDelta(content.clone()));
            }
        }
        ChatTurnEventKind::ThinkingDelta => {
            if let Some(content) = &event.content {
                updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                    TuiActivityKind::Chat,
                    "thinking",
                    content.clone(),
                )));
            }
        }
        ChatTurnEventKind::ThinkingSignatureDelta => {}
        ChatTurnEventKind::ToolCallStart => {
            updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                TuiActivityKind::Tool,
                "tool start",
                format!(
                    "{} {}",
                    event.tool_call_id.as_deref().unwrap_or(""),
                    event.tool_name.as_deref().unwrap_or("")
                ),
            )));
        }
        ChatTurnEventKind::ToolCallDelta => {
            if let Some(partial) = &event.partial_input_json {
                updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                    TuiActivityKind::Tool,
                    format!("tool args {}", event.tool_call_id.as_deref().unwrap_or("")),
                    partial.clone(),
                )));
            }
        }
        ChatTurnEventKind::ToolCallEnd => {
            updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                TuiActivityKind::Tool,
                "tool ready",
                format!(
                    "{} {}",
                    event.tool_call_id.as_deref().unwrap_or(""),
                    event.tool_name.as_deref().unwrap_or("")
                ),
            )));
        }
        ChatTurnEventKind::ToolResult => {
            let tool_name = event.tool_name.as_deref().unwrap_or("");
            let output = event.tool_output.as_ref().unwrap_or(&Value::Null);
            updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                TuiActivityKind::Tool,
                "tool result",
                tool_name.to_owned(),
            )));
            if tool_name == "ask_user" {
                if let Some(lines) = decision_request_lines(output) {
                    updates.push(TuiUpdate::ToolMessage {
                        title: Some(tool_name.to_owned()),
                        content: lines.join("\n"),
                    });
                } else {
                    updates.push(TuiUpdate::ToolMessage {
                        title: Some(tool_name.to_owned()),
                        content: format!("tool result: {}", compact_json(output)),
                    });
                }
            } else {
                updates.push(TuiUpdate::ToolMessage {
                    title: Some(tool_name.to_owned()),
                    content: format!("tool result: {}", compact_json(output)),
                });
            }
        }
        ChatTurnEventKind::Usage => {
            if let Some(usage) = &event.usage {
                updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                    TuiActivityKind::Context,
                    "usage",
                    format!(
                        "input={} output={} total={}",
                        usage.input_tokens, usage.output_tokens, usage.total_tokens
                    ),
                )));
            }
        }
        ChatTurnEventKind::ContextSnapshot => {
            push_context_status_updates(event, &mut updates);
        }
        ChatTurnEventKind::InteractionRequired | ChatTurnEventKind::InteractionResolved => {
            let interaction_id = event
                .metadata
                .get("interaction_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let interaction_kind = event
                .metadata
                .get("interaction_kind")
                .and_then(Value::as_str)
                .unwrap_or("interaction");
            let label = if event.kind == ChatTurnEventKind::InteractionRequired {
                "interaction required"
            } else {
                "interaction resolved"
            };
            updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                TuiActivityKind::Chat,
                label,
                format!("{interaction_kind} {interaction_id}"),
            )));
        }
        ChatTurnEventKind::RoundFinished => {
            push_context_status_updates(event, &mut updates);
            if let Some(response) = &event.response {
                *final_response = Some(response.clone());
                updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                    TuiActivityKind::Chat,
                    format!("round {} finished", event.round),
                    format!("{:?}", response.finish_reason),
                )));
            }
        }
        ChatTurnEventKind::Error => {
            let message = event.content.as_deref().unwrap_or("unknown error");
            if is_cancelled_chat_event(event) {
                updates.push(TuiUpdate::AssistantReplace("Cancelled.".to_owned()));
                updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                    TuiActivityKind::Cancellation,
                    "chat cancelled",
                    message.to_owned(),
                )));
            } else {
                updates.push(TuiUpdate::AssistantReplace(format!("Error: {message}")));
                updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                    TuiActivityKind::Error,
                    "chat error",
                    message.to_owned(),
                )));
            }
        }
        ChatTurnEventKind::Done => {
            let reason = event
                .metadata
                .get("stop_reason")
                .and_then(Value::as_str)
                .unwrap_or("done");
            updates.push(TuiUpdate::AssistantFinish);
            updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
                TuiActivityKind::Chat,
                "done",
                format!("{reason} in {} round(s)", event.round),
            )));
        }
    }
    updates
}

fn push_context_status_updates(event: &ChatTurnEvent, updates: &mut Vec<TuiUpdate>) {
    if let Some(status) = context_status_from_round_event(event) {
        updates.push(TuiUpdate::ContextStatus(status.clone()));
        updates.push(TuiUpdate::Activity(TuiActivityItem::with_detail(
            TuiActivityKind::Context,
            format!(
                "context: {}/{} tokens",
                status.token_estimate, status.max_input_tokens
            ),
            format!(
                "blocks={} omitted={} compacted={}",
                status.block_count, status.omitted_block_count, status.compacted
            ),
        )));
    }
}

pub(super) fn is_cancelled_chat_event(event: &ChatTurnEvent) -> bool {
    event.kind == ChatTurnEventKind::Error
        && event.metadata.get("code").and_then(Value::as_str) == Some("cancelled")
}

fn context_status_from_round_event(event: &ChatTurnEvent) -> Option<TuiContextStatus> {
    let snapshot = event.metadata.get("context_snapshot")?;
    let snapshot_id = snapshot.get("snapshot_id")?.as_str()?.to_owned();
    let token_estimate = u32_from_value(snapshot.get("token_estimate")?)?;
    let max_input_tokens = u32_from_value(snapshot.get("max_input_tokens")?)?;
    let block_count = usize_from_value(snapshot.get("block_count")?)?;
    let omitted_block_count = u32_from_value(snapshot.get("omitted_block_count")?)?;
    let compacted = snapshot
        .get("compacted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let compaction_strategy = event
        .metadata
        .get("compaction")
        .and_then(|value| value.get("strategy"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Some(TuiContextStatus {
        snapshot_id,
        token_estimate,
        max_input_tokens,
        block_count,
        omitted_block_count,
        compacted,
        compaction_strategy,
    })
}

fn u32_from_value(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|value| u32::try_from(value).ok())
}

fn usize_from_value(value: &Value) -> Option<usize> {
    value.as_u64().and_then(|value| usize::try_from(value).ok())
}

fn decision_request_lines(output: &Value) -> Option<Vec<String>> {
    let object = output.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("decision_request") {
        return None;
    }
    let title = object.get("title").and_then(Value::as_str)?.trim();
    if title.is_empty() {
        return None;
    }
    let options = object.get("options").and_then(Value::as_array)?;
    if options.len() < 2 {
        return None;
    }

    let mut lines = vec![format!("decision: {title}")];
    if let Some(context) = object
        .get("context")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|context| !context.is_empty())
    {
        lines.push(format!("context: {context}"));
    }
    for (index, option) in options.iter().enumerate() {
        let option = option.as_object()?;
        let label = option.get("label").and_then(Value::as_str)?.trim();
        if label.is_empty() {
            return None;
        }
        let description = option
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|description| !description.is_empty());
        let recommended = option
            .get("recommended")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let marker = if recommended { " [recommended]" } else { "" };
        match description {
            Some(description) => {
                lines.push(format!("  {}. {label} - {description}{marker}", index + 1))
            }
            None => lines.push(format!("  {}. {label}{marker}", index + 1)),
        }
    }
    lines.push("reply with your choice to continue".to_owned());
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{data::TuiActivityKind, test_support::test_state};
    use agent_chat::{ChatTurnEvent, ChatTurnEventKind};
    use serde_json::json;

    #[tokio::test]
    async fn tui_applies_shared_agent_chat_turn_event_fixture() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "unused").await;
        state.clear_output();

        let events: Vec<ChatTurnEvent> =
            serde_json::from_str(include_str!("../../../../fixtures/chat/turn_events.json"))
                .expect("shared chat turn events fixture");
        let mut assistant_text = String::new();
        let mut final_response = None;
        for event in &events {
            apply_chat_event_to_tui(&mut state, event, &mut assistant_text, &mut final_response);
        }

        assert_eq!(assistant_text, "Checking ");
        assert!(final_response.is_some());
        assert!(
            state
                .events
                .iter()
                .any(|line| line.contains("tool start: call_1 get_holdings"))
        );
        assert!(
            state
                .events
                .iter()
                .any(|line| line.contains("usage: input=11 output=7 total=18"))
        );
        assert!(
            state
                .events
                .iter()
                .any(|line| line.contains("round 1 finished"))
        );
    }

    #[tokio::test]
    async fn tui_tracks_and_renders_chat_context_status() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "unused").await;
        state.clear_output();
        let event = ChatTurnEvent {
            kind: ChatTurnEventKind::RoundFinished,
            content: None,
            response: None,
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            tool_output: None,
            usage: None,
            round: 1,
            metadata: json!({
                "context_snapshot": {
                    "snapshot_id": "ctx_test",
                    "token_estimate": 123,
                    "max_input_tokens": 200,
                    "block_count": 4,
                    "omitted_block_count": 2,
                    "compacted": true
                },
                "compaction": {
                    "strategy": "deterministic_recent_messages"
                }
            }),
        };
        let mut assistant_text = String::new();
        let mut final_response = None;

        apply_chat_event_to_tui(&mut state, &event, &mut assistant_text, &mut final_response);

        let status = state.context_status.as_ref().expect("context status");
        assert_eq!(status.snapshot_id, "ctx_test");
        assert_eq!(status.token_estimate, 123);
        assert_eq!(status.max_input_tokens, 200);
        assert_eq!(status.block_count, 4);
        assert_eq!(status.omitted_block_count, 2);
        assert!(status.compacted);
        assert!(
            state
                .events
                .iter()
                .any(|line| line.contains("context: 123/200 tokens"))
        );
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Context && activity.title == "context: 123/200 tokens"
        }));
        let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
        assert!(rendered.contains("Chat context"));
        assert!(rendered.contains("tokens  123/200"));
    }

    #[tokio::test]
    async fn tui_renders_shared_ask_user_turn_fixture_as_decision_options() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "unused").await;
        state.clear_output();

        let events: Vec<ChatTurnEvent> = serde_json::from_str(include_str!(
            "../../../../fixtures/chat/ask_user_turn_events.json"
        ))
        .expect("shared ask_user turn events fixture");
        let mut assistant_text = String::new();
        let mut final_response = None;
        for event in &events {
            apply_chat_event_to_tui(&mut state, event, &mut assistant_text, &mut final_response);
        }

        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("decision: Implementation path"))
        );
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("1. Context transcript"))
        );
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("[recommended]"))
        );
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("reply with your choice to continue"))
        );
    }
}
