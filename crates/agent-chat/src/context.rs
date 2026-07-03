use agent_core::{
    CompactionRecord, ContextBlock, ContextBlockKind, ContextPolicy, ContextSnapshot,
    PROTOCOL_VERSION, ToolSpec,
};
use agent_llm::{LlmMessage, LlmRequest, LlmRole};
use serde_json::{Value, json};

use crate::{ChatError, ChatTurnState, state::llm_metadata};

#[derive(Debug, Clone)]
pub(crate) struct PreparedContext {
    pub(crate) request: LlmRequest,
}

pub(crate) fn prepare_llm_request(state: &mut ChatTurnState) -> Result<PreparedContext, ChatError> {
    let policy = state.context_policy.clone();
    let original_messages = state.messages.clone();
    let before_blocks = context_blocks(&original_messages, &state.tools);
    let before_tokens = total_tokens(&before_blocks);
    let before_hash = blocks_hash(&before_blocks);
    let budget = effective_input_budget(&policy);

    let (messages, omitted_block_count, summary) =
        if policy.compact_when_over_budget && before_tokens > budget {
            compact_messages(&original_messages, policy.preserve_recent_messages)
        } else {
            (original_messages, 0, None)
        };

    if omitted_block_count > 0 {
        state.messages = messages.clone();
    }

    let blocks = context_blocks(&messages, &state.tools);
    let token_estimate = total_tokens(&blocks);
    let content_hash = blocks_hash(&blocks);
    let snapshot = ContextSnapshot::new(
        format!(
            "ctx_{}",
            content_hash
                .strip_prefix("blake3:")
                .unwrap_or(&content_hash)
        ),
        content_hash.clone(),
        token_estimate,
        budget,
        omitted_block_count,
        omitted_block_count > 0,
        blocks,
        json!({
            "turn_id": state.turn_id,
            "session_id": state.session_id,
            "thread_id": state.thread_id,
            "agent_id": state.agent_id,
            "round": state.round,
        }),
    );
    let compaction = summary.map(|summary| CompactionRecord {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        before_snapshot_hash: before_hash,
        after_snapshot_hash: snapshot.content_hash.clone(),
        omitted_block_count,
        strategy: "deterministic_recent_messages".to_owned(),
        summary,
        metadata: json!({
            "preserve_recent_messages": policy.preserve_recent_messages,
            "max_input_tokens": policy.max_input_tokens,
            "reserve_output_tokens": policy.reserve_output_tokens,
        }),
    });
    state.context_snapshot = Some(snapshot.clone());
    state.compaction = compaction.clone();

    let mut metadata = llm_metadata(state);
    if let Some(object) = metadata.as_object_mut() {
        object.insert(
            "context_snapshot".to_owned(),
            serde_json::to_value(snapshot_summary(&snapshot)).map_err(|error| {
                ChatError::validation(format!(
                    "failed to encode context snapshot metadata: {error}"
                ))
            })?,
        );
        if let Some(compaction) = &compaction {
            object.insert(
                "compaction".to_owned(),
                serde_json::to_value(compaction).map_err(|error| {
                    ChatError::validation(format!("failed to encode compaction metadata: {error}"))
                })?,
            );
        }
    }

    Ok(PreparedContext {
        request: LlmRequest {
            protocol_version: state.protocol_version.clone(),
            provider: state.provider.clone(),
            model: state.model.clone(),
            messages,
            temperature: state.temperature,
            max_output_tokens: state.max_output_tokens,
            tools: state.tools.clone(),
            response_format: None,
            metadata,
        },
    })
}

pub(crate) fn build_llm_request_without_state_update(state: &ChatTurnState) -> LlmRequest {
    let mut state = state.clone();
    prepare_llm_request(&mut state)
        .map(|prepared| prepared.request)
        .unwrap_or_else(|_| LlmRequest {
            protocol_version: state.protocol_version.clone(),
            provider: state.provider.clone(),
            model: state.model.clone(),
            messages: state.messages.clone(),
            temperature: state.temperature,
            max_output_tokens: state.max_output_tokens,
            tools: state.tools.clone(),
            response_format: None,
            metadata: llm_metadata(&state),
        })
}

fn context_blocks(messages: &[LlmMessage], tools: &[ToolSpec]) -> Vec<ContextBlock> {
    let mut blocks = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let content = json!({
            "role": message.role,
            "content": message.content,
            "name": message.name,
        });
        blocks.push(ContextBlock {
            block_id: format!("message_{index}"),
            kind: ContextBlockKind::Message,
            source: "chat.messages".to_owned(),
            priority: message_priority(message),
            token_estimate: token_estimate(&content),
            content_hash: value_hash(&content),
            content,
            metadata: json!({"index": index}),
        });
    }
    for (index, tool) in tools.iter().enumerate() {
        let content = serde_json::to_value(tool).unwrap_or_else(|_| json!({}));
        blocks.push(ContextBlock {
            block_id: format!("tool_{index}_{}", tool.name),
            kind: ContextBlockKind::ToolSchema,
            source: "chat.tools".to_owned(),
            priority: 50,
            token_estimate: token_estimate(&content),
            content_hash: value_hash(&content),
            content,
            metadata: json!({"index": index, "tool_name": tool.name}),
        });
    }
    blocks
}

fn compact_messages(
    messages: &[LlmMessage],
    preserve_recent_messages: usize,
) -> (Vec<LlmMessage>, u32, Option<String>) {
    if messages.len() <= preserve_recent_messages.saturating_add(1) {
        return (messages.to_vec(), 0, None);
    }

    let system_prefix_len = messages
        .iter()
        .take_while(|message| matches!(message.role, LlmRole::System))
        .count();
    let protected_prefix = &messages[..system_prefix_len];
    let rest = &messages[system_prefix_len..];
    if rest.len() <= preserve_recent_messages {
        return (messages.to_vec(), 0, None);
    }

    let omit_count = rest.len() - preserve_recent_messages;
    let omitted = &rest[..omit_count];
    let preserved = &rest[omit_count..];
    let summary = summarize_messages(omitted);
    let mut compacted = protected_prefix.to_vec();
    compacted.push(LlmMessage {
        role: LlmRole::System,
        content: Value::String(summary.clone()),
        name: Some("context_compaction".to_owned()),
        metadata: json!({
            "context_compaction": true,
            "omitted_message_count": omit_count,
        }),
    });
    compacted.extend_from_slice(preserved);
    (
        compacted,
        u32::try_from(omit_count).unwrap_or(u32::MAX),
        Some(summary),
    )
}

fn summarize_messages(messages: &[LlmMessage]) -> String {
    let mut role_counts = serde_json::Map::new();
    for message in messages {
        let key = format!("{:?}", message.role).to_ascii_lowercase();
        let count = role_counts.get(&key).and_then(Value::as_u64).unwrap_or(0) + 1;
        role_counts.insert(key, json!(count));
    }
    let previews = messages
        .iter()
        .take(8)
        .enumerate()
        .map(|(index, message)| {
            format!(
                "{}. {:?}: {}",
                index + 1,
                message.role,
                truncate(&content_text(&message.content), 160)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Context compaction summary: {} older messages were compacted. Role counts: {}.\nPreserved facts are represented by these previews:\n{}",
        messages.len(),
        Value::Object(role_counts),
        previews
    )
}

fn snapshot_summary(snapshot: &ContextSnapshot) -> Value {
    json!({
        "snapshot_id": snapshot.snapshot_id,
        "content_hash": snapshot.content_hash,
        "token_estimate": snapshot.token_estimate,
        "max_input_tokens": snapshot.max_input_tokens,
        "omitted_block_count": snapshot.omitted_block_count,
        "compacted": snapshot.compacted,
        "block_count": snapshot.blocks.len(),
    })
}

fn effective_input_budget(policy: &ContextPolicy) -> u32 {
    policy
        .max_input_tokens
        .saturating_sub(policy.reserve_output_tokens)
        .max(1)
}

fn total_tokens(blocks: &[ContextBlock]) -> u32 {
    blocks.iter().fold(0_u32, |total, block| {
        total.saturating_add(block.token_estimate)
    })
}

fn blocks_hash(blocks: &[ContextBlock]) -> String {
    value_hash(
        &serde_json::to_value(
            blocks
                .iter()
                .map(|block| (&block.block_id, &block.content_hash))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| json!([])),
    )
}

fn message_priority(message: &LlmMessage) -> i32 {
    match message.role {
        LlmRole::System => 100,
        LlmRole::User => 80,
        LlmRole::Assistant => 70,
        LlmRole::Tool => 60,
    }
}

fn token_estimate(value: &Value) -> u32 {
    let chars = value.to_string().chars().count();
    u32::try_from(chars / 4 + 1).unwrap_or(u32::MAX)
}

fn value_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    format!("blake3:{}", blake3::hash(&bytes).to_hex())
}

fn content_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
