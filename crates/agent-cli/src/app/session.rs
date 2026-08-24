use agent_chat::{ChatTurnEvent, ChatTurnEventKind};
use agent_core::{
    AgentSessionStore, ChatTranscriptEvent, ChatTranscriptEventKind, CompactionRecord,
    ContextCheckpoint, ContextSnapshot, PROTOCOL_VERSION, SessionId, SessionRecord, StepKind,
    StepRecord, ThreadId, ThreadRecord,
};
use agent_runtime::RunOutcome;
use miette::{IntoDiagnostic, Result, miette};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Serialize)]
pub(crate) struct SessionCreateReport {
    pub(crate) session: SessionRecord,
    pub(crate) thread: ThreadRecord,
}

#[derive(Debug, Serialize)]
pub(crate) struct SessionShowReport {
    pub(crate) session: SessionRecord,
    pub(crate) threads: Vec<ThreadWithSteps>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ThreadWithSteps {
    pub(crate) thread: ThreadRecord,
    pub(crate) steps: Vec<StepRecord>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ThreadForkReport {
    pub(crate) session_id: String,
    pub(crate) parent_thread_id: String,
    pub(crate) thread: ThreadRecord,
}

#[derive(Debug, Serialize)]
pub(crate) struct HttpSessionCreateResponse {
    pub(crate) session: SessionRecord,
    pub(crate) thread: ThreadRecord,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpSessionCreateParams {
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) metadata: Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HttpThreadForkParams {
    pub(crate) parent_thread_id: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) metadata: Value,
}

pub(crate) async fn create_session(
    store: &dyn AgentSessionStore,
    title: String,
) -> Result<SessionCreateReport> {
    let session = SessionRecord::new(title.clone(), json!({}));
    let thread = ThreadRecord::root(session.session_id.clone(), Some(title), json!({}));
    store
        .create_session(session.clone())
        .await
        .into_diagnostic()?;
    store
        .create_thread(thread.clone())
        .await
        .into_diagnostic()?;
    Ok(SessionCreateReport { session, thread })
}

pub(crate) fn run_metadata(session: Option<&str>, thread: Option<&str>) -> Value {
    json!({
        "session_id": session,
        "thread_id": thread,
    })
}

pub(crate) async fn record_session_step(
    store: &dyn AgentSessionStore,
    thread_id: Option<&str>,
    outcome: &RunOutcome,
) -> Result<()> {
    let Some(thread_id) = thread_id else {
        return Ok(());
    };
    let thread_id = ThreadId(thread_id.to_owned());
    let thread = store
        .get_thread(&thread_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("thread '{}' was not found", thread_id.0))?;
    let step = StepRecord::agent_run(
        thread.thread_id,
        outcome.result.run_id.clone(),
        outcome.result.summary.clone(),
        json!({
            "agent_id": outcome.result.agent_id.clone(),
            "status": outcome.result.status.clone(),
        }),
    );
    store.create_step(step).await.into_diagnostic()
}

pub(crate) async fn ensure_thread(
    store: &dyn AgentSessionStore,
    thread_id: Option<&str>,
) -> Result<Option<ThreadId>> {
    let Some(thread_id) = thread_id else {
        return Ok(None);
    };
    let thread_id = ThreadId(thread_id.to_owned());
    store
        .get_thread(&thread_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("thread '{}' was not found", thread_id.0))?;
    Ok(Some(thread_id))
}

pub(crate) async fn record_chat_event_step(
    store: &dyn AgentSessionStore,
    thread_id: &ThreadId,
    event: &ChatTurnEvent,
) -> Result<()> {
    if event.kind == ChatTurnEventKind::ContextSnapshot {
        persist_context_checkpoint(store, thread_id, event).await?;
    }
    if let Some(transcript_event) = chat_transcript_event(thread_id, event) {
        append_chat_transcript_event(store, transcript_event).await?;
    }
    if let Some(step) = chat_event_step(thread_id.clone(), event) {
        store.create_step(step).await.into_diagnostic()?;
    }
    Ok(())
}

async fn append_chat_transcript_event(
    store: &dyn AgentSessionStore,
    mut event: ChatTranscriptEvent,
) -> Result<()> {
    for _ in 0..3 {
        let events = store
            .list_chat_transcript_events_after(&event.thread_id, 0)
            .await
            .into_diagnostic()?;
        let expected_sequence = events.last().map(|event| event.sequence).unwrap_or(0);
        event.sequence = expected_sequence.saturating_add(1);
        if store
            .append_chat_transcript_event(event.clone(), expected_sequence)
            .await
            .into_diagnostic()?
        {
            return Ok(());
        }
    }
    Err(miette!(
        "chat transcript sequence changed while appending event '{}'",
        event.event_id
    ))
}

async fn persist_context_checkpoint(
    store: &dyn AgentSessionStore,
    thread_id: &ThreadId,
    event: &ChatTurnEvent,
) -> Result<()> {
    let Some(snapshot) = event
        .metadata
        .get("context_snapshot")
        .cloned()
        .and_then(|value| serde_json::from_value::<ContextSnapshot>(value).ok())
    else {
        return Ok(());
    };
    if !snapshot.compacted {
        return Ok(());
    }
    let compaction = event
        .metadata
        .get("compaction")
        .cloned()
        .and_then(|value| serde_json::from_value::<CompactionRecord>(value).ok());
    let semantic_basis_digest = event
        .metadata
        .get("context_epoch")
        .and_then(|value| value.get("semantic_basis_digest"))
        .and_then(Value::as_str)
        .unwrap_or("legacy-context-basis")
        .to_owned();
    let sequence_hint = event
        .metadata
        .get("transcript_sequence")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let body = compaction
        .as_ref()
        .map(|compaction| compaction.summary.clone())
        .filter(|body| !body.is_empty())
        .unwrap_or_else(|| {
            "Session continuity was compacted; retained blocks are evidence only.".to_owned()
        });
    let checkpoint_id = snapshot.snapshot_id.clone();
    for _ in 0..3 {
        let events = store
            .list_chat_transcript_events_after(thread_id, 0)
            .await
            .into_diagnostic()?;
        let expected_sequence = events.last().map(|event| event.sequence).unwrap_or(0);
        let previous_checkpoint_id = store
            .get_context_checkpoint(thread_id)
            .await
            .into_diagnostic()?
            .map(|checkpoint| checkpoint.checkpoint_id);
        let checkpoint = ContextCheckpoint {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            checkpoint_id: checkpoint_id.clone(),
            thread_id: thread_id.clone(),
            previous_checkpoint_id,
            replaces_through_seq: sequence_hint.min(expected_sequence),
            archive_through_seq: sequence_hint.min(expected_sequence),
            semantic_basis_digest: semantic_basis_digest.clone(),
            protected_content_digest: snapshot.content_hash.clone(),
            replacement_plan_digest: compaction
                .as_ref()
                .map(|compaction| compaction.after_snapshot_hash.clone())
                .unwrap_or_else(|| snapshot.content_hash.clone()),
            body: body.clone(),
            archive_records: Vec::new(),
            retained_entries: snapshot
                .blocks
                .iter()
                .filter_map(|block| serde_json::to_value(block).ok())
                .collect(),
            created_at: snapshot.created_at,
        };
        if store
            .commit_context_checkpoint(checkpoint, expected_sequence)
            .await
            .into_diagnostic()?
            .is_some()
        {
            return Ok(());
        }
    }
    Err(miette!(
        "context checkpoint '{}' could not pass the transcript CAS",
        checkpoint_id
    ))
}

fn chat_transcript_event(
    thread_id: &ThreadId,
    event: &ChatTurnEvent,
) -> Option<ChatTranscriptEvent> {
    let kind = match event.kind {
        ChatTurnEventKind::Started => ChatTranscriptEventKind::TurnStarted,
        ChatTurnEventKind::ToolCallEnd => ChatTranscriptEventKind::ToolCall,
        ChatTurnEventKind::ToolResult => ChatTranscriptEventKind::ToolResult,
        ChatTurnEventKind::InteractionResolved => ChatTranscriptEventKind::InteractionResult,
        ChatTurnEventKind::RoundFinished => ChatTranscriptEventKind::AssistantMessage,
        ChatTurnEventKind::Done => ChatTranscriptEventKind::TurnCompleted,
        ChatTurnEventKind::Error => ChatTranscriptEventKind::TurnFailed,
        _ => return None,
    };
    let payload = serde_json::to_value(event).ok()?;
    let bytes = serde_json::to_vec(&payload).ok()?;
    let content_hash = format!("blake3:{}", blake3::hash(&bytes).to_hex());
    Some(ChatTranscriptEvent {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        event_id: format!("chat_event_{content_hash}"),
        thread_id: thread_id.clone(),
        sequence: 0,
        turn_id: event
            .metadata
            .get("turn_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        kind,
        payload,
        content_hash,
        created_at: time::OffsetDateTime::now_utc(),
    })
}

fn chat_event_step(thread_id: ThreadId, event: &ChatTurnEvent) -> Option<StepRecord> {
    let payload = json!({"event": event});
    match event.kind {
        ChatTurnEventKind::RoundFinished => {
            let status = event
                .metadata
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("finished");
            Some(StepRecord::new(
                thread_id,
                StepKind::LlmRound,
                None,
                Some(format!("chat round {} {status}", event.round)),
                payload,
            ))
        }
        ChatTurnEventKind::ToolResult => Some(StepRecord::new(
            thread_id,
            StepKind::ToolCall,
            None,
            event
                .tool_name
                .as_ref()
                .map(|tool_name| format!("chat tool {tool_name}")),
            payload,
        )),
        ChatTurnEventKind::Done => Some(StepRecord::new(
            thread_id,
            StepKind::StateUpdate,
            None,
            Some("chat turn done".to_owned()),
            payload,
        )),
        ChatTurnEventKind::Error => Some(StepRecord::new(
            thread_id,
            StepKind::StateUpdate,
            None,
            Some("chat turn error".to_owned()),
            payload,
        )),
        _ => None,
    }
}

pub(crate) async fn show_session(
    store: &dyn AgentSessionStore,
    session_id: SessionId,
) -> Result<SessionShowReport> {
    let session = store
        .get_session(&session_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("session '{}' was not found", session_id.0))?;
    let mut threads = Vec::new();
    for thread in store
        .list_threads(&session.session_id)
        .await
        .into_diagnostic()?
    {
        let steps = store
            .list_steps(&thread.thread_id)
            .await
            .into_diagnostic()?;
        threads.push(ThreadWithSteps { thread, steps });
    }
    Ok(SessionShowReport { session, threads })
}

pub(crate) async fn fork_thread(
    store: &dyn AgentSessionStore,
    session_id: SessionId,
    parent_thread_id: ThreadId,
    title: Option<String>,
) -> Result<ThreadForkReport> {
    store
        .get_session(&session_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("session '{}' was not found", session_id.0))?;
    let parent = store
        .get_thread(&parent_thread_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("thread '{}' was not found", parent_thread_id.0))?;
    if parent.session_id != session_id {
        return Err(miette!(
            "thread '{}' does not belong to session '{}'",
            parent_thread_id.0,
            session_id.0
        ));
    }
    let thread = ThreadRecord::fork(
        session_id.clone(),
        parent_thread_id.clone(),
        title,
        json!({}),
    );
    store
        .create_thread(thread.clone())
        .await
        .into_diagnostic()?;
    Ok(ThreadForkReport {
        session_id: session_id.0,
        parent_thread_id: parent_thread_id.0,
        thread,
    })
}
