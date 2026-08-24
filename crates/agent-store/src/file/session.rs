use super::*;

pub struct FileSessionStore {
    root: Utf8PathBuf,
    chat_state_lock: Arc<Mutex<()>>,
}

impl FileSessionStore {
    pub async fn new(root: impl Into<Utf8PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        for dir in [
            session_dir(&root),
            thread_dir(&root),
            step_dir(&root),
            chat_dir(&root),
        ] {
            fs_err::tokio::create_dir_all(dir)
                .await
                .map_err(map_store_err)?;
        }
        Ok(Self {
            root,
            chat_state_lock: Arc::new(Mutex::new(())),
        })
    }

    fn session_path_for(&self, session_id: &SessionId) -> Utf8PathBuf {
        session_dir(&self.root).join(format!("{}.json", session_id.0))
    }

    fn thread_path_for(&self, thread_id: &ThreadId) -> Utf8PathBuf {
        thread_dir(&self.root).join(format!("{}.json", thread_id.0))
    }

    fn step_path_for(&self, step: &StepRecord) -> Utf8PathBuf {
        step_dir(&self.root).join(format!("{}.json", step.step_id.0))
    }

    fn chat_path_for(&self, thread_id: &ThreadId) -> Utf8PathBuf {
        chat_dir(&self.root).join(format!(
            "{}.json",
            blake3::hash(thread_id.0.as_bytes()).to_hex()
        ))
    }
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct FileChatLog {
    #[serde(default)]
    events: Vec<ChatTranscriptEvent>,
    #[serde(default)]
    checkpoint: Option<ContextCheckpoint>,
}

impl FileSessionStore {
    async fn read_chat_log(&self, thread_id: &ThreadId) -> Result<FileChatLog, StoreError> {
        Ok(read_optional_json(&self.chat_path_for(thread_id))
            .await?
            .unwrap_or_default())
    }

    async fn write_chat_log(
        &self,
        thread_id: &ThreadId,
        log: &FileChatLog,
    ) -> Result<(), StoreError> {
        write_json(&self.chat_path_for(thread_id), log).await
    }
}

#[async_trait]
impl AgentSessionStore for FileSessionStore {
    async fn create_session(&self, session: SessionRecord) -> Result<(), StoreError> {
        write_json(&self.session_path_for(&session.session_id), &session).await
    }

    async fn list_sessions(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let mut sessions = read_json_records::<SessionRecord>(&session_dir(&self.root)).await?;
        sessions.sort_by_key(|session| session.updated_at);
        sessions.reverse();
        Ok(sessions)
    }

    async fn get_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionRecord>, StoreError> {
        read_optional_json(&self.session_path_for(session_id)).await
    }

    async fn create_thread(&self, thread: ThreadRecord) -> Result<(), StoreError> {
        write_json(&self.thread_path_for(&thread.thread_id), &thread).await
    }

    async fn list_threads(&self, session_id: &SessionId) -> Result<Vec<ThreadRecord>, StoreError> {
        let mut threads = read_json_records::<ThreadRecord>(&thread_dir(&self.root))
            .await?
            .into_iter()
            .filter(|thread| thread.session_id == *session_id)
            .collect::<Vec<_>>();
        threads.sort_by_key(|thread| thread.created_at);
        Ok(threads)
    }

    async fn get_thread(&self, thread_id: &ThreadId) -> Result<Option<ThreadRecord>, StoreError> {
        read_optional_json(&self.thread_path_for(thread_id)).await
    }

    async fn create_step(&self, step: StepRecord) -> Result<(), StoreError> {
        write_json(&self.step_path_for(&step), &step).await
    }

    async fn list_steps(&self, thread_id: &ThreadId) -> Result<Vec<StepRecord>, StoreError> {
        let mut steps = read_json_records::<StepRecord>(&step_dir(&self.root))
            .await?
            .into_iter()
            .filter(|step| step.thread_id == *thread_id)
            .collect::<Vec<_>>();
        steps.sort_by_key(|step| step.created_at);
        Ok(steps)
    }

    async fn append_chat_transcript_event(
        &self,
        event: ChatTranscriptEvent,
        expected_sequence: u64,
    ) -> Result<bool, StoreError> {
        let _guard = self.chat_state_lock.lock().await;
        let thread_id = event.thread_id.clone();
        let mut log = self.read_chat_log(&thread_id).await?;
        if let Some(existing) = log
            .events
            .iter()
            .find(|item| item.event_id == event.event_id)
        {
            if existing.content_hash == event.content_hash && existing.payload == event.payload {
                return Ok(true);
            }
            return Err(StoreError::new(format!(
                "chat transcript event '{}' already exists with different content",
                event.event_id
            )));
        }
        let current_sequence = log.events.last().map(|event| event.sequence).unwrap_or(0);
        if current_sequence != expected_sequence
            || event.sequence != expected_sequence.saturating_add(1)
        {
            return Ok(false);
        }
        log.events.push(event);
        self.write_chat_log(&thread_id, &log).await?;
        Ok(true)
    }

    async fn list_chat_transcript_events_after(
        &self,
        thread_id: &ThreadId,
        after: u64,
    ) -> Result<Vec<ChatTranscriptEvent>, StoreError> {
        let _guard = self.chat_state_lock.lock().await;
        let mut events = self
            .read_chat_log(thread_id)
            .await?
            .events
            .into_iter()
            .filter(|event| event.sequence > after)
            .collect::<Vec<_>>();
        events.sort_by_key(|event| event.sequence);
        Ok(events)
    }

    async fn get_context_checkpoint(
        &self,
        thread_id: &ThreadId,
    ) -> Result<Option<ContextCheckpoint>, StoreError> {
        let _guard = self.chat_state_lock.lock().await;
        Ok(self.read_chat_log(thread_id).await?.checkpoint)
    }

    async fn commit_context_checkpoint(
        &self,
        checkpoint: ContextCheckpoint,
        expected_sequence: u64,
    ) -> Result<Option<ContextCheckpointCommit>, StoreError> {
        let _guard = self.chat_state_lock.lock().await;
        let thread_id = checkpoint.thread_id.clone();
        let mut log = self.read_chat_log(&thread_id).await?;
        if let Some(existing) = &log.checkpoint {
            if existing.checkpoint_id == checkpoint.checkpoint_id {
                let event = log
                    .events
                    .iter()
                    .find(|event| {
                        event.kind == ChatTranscriptEventKind::ContextCheckpointed
                            && event
                                .payload
                                .get("checkpoint_id")
                                .and_then(serde_json::Value::as_str)
                                == Some(checkpoint.checkpoint_id.as_str())
                    })
                    .cloned()
                    .ok_or_else(|| StoreError::new("checkpoint exists without checkpoint event"))?;
                return Ok(Some(ContextCheckpointCommit {
                    checkpoint: existing.clone(),
                    event,
                }));
            }
        }
        let current_sequence = log.events.last().map(|event| event.sequence).unwrap_or(0);
        if current_sequence != expected_sequence {
            return Ok(None);
        }
        let current_checkpoint_id = log
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_id.clone());
        if checkpoint.previous_checkpoint_id != current_checkpoint_id {
            return Ok(None);
        }
        let sequence = expected_sequence.saturating_add(1);
        let event = ChatTranscriptEvent {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            event_id: format!("checkpoint:{}", checkpoint.checkpoint_id),
            thread_id: thread_id.clone(),
            sequence,
            turn_id: None,
            kind: ChatTranscriptEventKind::ContextCheckpointed,
            payload: serde_json::json!({
                "checkpoint_id": checkpoint.checkpoint_id,
                "replaces_through_seq": checkpoint.replaces_through_seq,
                "archive_through_seq": checkpoint.archive_through_seq,
                "semantic_basis_digest": checkpoint.semantic_basis_digest,
                "replacement_plan_digest": checkpoint.replacement_plan_digest,
            }),
            content_hash: checkpoint.replacement_plan_digest.clone(),
            created_at: checkpoint.created_at,
        };
        log.events.push(event.clone());
        log.checkpoint = Some(checkpoint.clone());
        self.write_chat_log(&thread_id, &log).await?;
        Ok(Some(ContextCheckpointCommit { checkpoint, event }))
    }
}
