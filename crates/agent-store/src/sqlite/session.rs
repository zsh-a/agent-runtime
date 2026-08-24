use super::*;

fn chat_event_from_step(step: &StepRecord) -> Option<ChatTranscriptEvent> {
    if step.payload.get("chat_transcript_event") != Some(&serde_json::Value::Bool(true)) {
        return None;
    }
    serde_json::from_value(step.payload.get("event")?.clone()).ok()
}

fn checkpoint_from_step(step: &StepRecord) -> Option<ContextCheckpoint> {
    chat_event_from_step(step)
        .filter(|event| event.kind == ChatTranscriptEventKind::ContextCheckpointed)
        .and_then(|_| serde_json::from_value(step.payload.get("checkpoint")?.clone()).ok())
}

fn chat_step(event: &ChatTranscriptEvent, checkpoint: Option<&ContextCheckpoint>) -> StepRecord {
    StepRecord {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        step_id: StepId(event.event_id.clone()),
        thread_id: event.thread_id.clone(),
        kind: StepKind::StateUpdate,
        run_id: None,
        summary: Some(format!("chat transcript {:?}", event.kind)),
        payload: serde_json::json!({
            "chat_transcript_event": true,
            "event": event,
            "checkpoint": checkpoint,
        }),
        created_at: event.created_at,
    }
}

async fn read_chat_steps(
    connection: &mut SqliteConnection,
    thread_id: &ThreadId,
) -> Result<Vec<StepRecord>, StoreError> {
    let rows = sqlx::query(
        r#"
        SELECT record_json FROM agent_steps
        WHERE thread_id = ?
        ORDER BY created_at_sort ASC
        "#,
    )
    .bind(&thread_id.0)
    .fetch_all(&mut *connection)
    .await
    .map_err(map_sqlx_err)?;
    rows.into_iter()
        .map(|row| decode_record(row.get::<String, _>("record_json")))
        .collect()
}

#[async_trait]
impl AgentSessionStore for SqliteStore {
    async fn create_session(&self, session: SessionRecord) -> Result<(), StoreError> {
        let record_json = encode_record(&session)?;
        sqlx::query(
            r#"
            INSERT INTO agent_sessions(session_id, updated_at_sort, record_json)
            VALUES (?, ?, ?)
            ON CONFLICT(session_id) DO UPDATE SET
                updated_at_sort = excluded.updated_at_sort,
                record_json = excluded.record_json
            "#,
        )
        .bind(&session.session_id.0)
        .bind(sort_key(session.updated_at)?)
        .bind(record_json)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT record_json FROM agent_sessions
            ORDER BY updated_at_sort DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }

    async fn get_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionRecord>, StoreError> {
        let row = sqlx::query("SELECT record_json FROM agent_sessions WHERE session_id = ?")
            .bind(&session_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }

    async fn create_thread(&self, thread: ThreadRecord) -> Result<(), StoreError> {
        let record_json = encode_record(&thread)?;
        sqlx::query(
            r#"
            INSERT INTO agent_threads(thread_id, session_id, created_at_sort, record_json)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(thread_id) DO UPDATE SET
                session_id = excluded.session_id,
                created_at_sort = excluded.created_at_sort,
                record_json = excluded.record_json
            "#,
        )
        .bind(&thread.thread_id.0)
        .bind(&thread.session_id.0)
        .bind(sort_key(thread.created_at)?)
        .bind(record_json)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn list_threads(&self, session_id: &SessionId) -> Result<Vec<ThreadRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT record_json FROM agent_threads
            WHERE session_id = ?
            ORDER BY created_at_sort ASC
            "#,
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }

    async fn get_thread(&self, thread_id: &ThreadId) -> Result<Option<ThreadRecord>, StoreError> {
        let row = sqlx::query("SELECT record_json FROM agent_threads WHERE thread_id = ?")
            .bind(&thread_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }

    async fn create_step(&self, step: StepRecord) -> Result<(), StoreError> {
        let record_json = encode_record(&step)?;
        sqlx::query(
            r#"
            INSERT INTO agent_steps(step_id, thread_id, created_at_sort, record_json)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(step_id) DO UPDATE SET
                thread_id = excluded.thread_id,
                created_at_sort = excluded.created_at_sort,
                record_json = excluded.record_json
            "#,
        )
        .bind(&step.step_id.0)
        .bind(&step.thread_id.0)
        .bind(sort_key(step.created_at)?)
        .bind(record_json)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn list_steps(&self, thread_id: &ThreadId) -> Result<Vec<StepRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT record_json FROM agent_steps
            WHERE thread_id = ?
              AND COALESCE(json_extract(record_json, '$.payload.chat_transcript_event'), 0) = 0
            ORDER BY created_at_sort ASC
            "#,
        )
        .bind(&thread_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }

    async fn append_chat_transcript_event(
        &self,
        event: ChatTranscriptEvent,
        expected_sequence: u64,
    ) -> Result<bool, StoreError> {
        let mut connection = self.pool.acquire().await.map_err(map_sqlx_err)?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .map_err(map_sqlx_err)?;
        let result = async {
            let steps = read_chat_steps(&mut connection, &event.thread_id).await?;
            let mut events = steps
                .iter()
                .filter_map(chat_event_from_step)
                .collect::<Vec<_>>();
            events.sort_by_key(|event| event.sequence);
            if let Some(existing) = events.iter().find(|item| item.event_id == event.event_id) {
                if existing.content_hash == event.content_hash && existing.payload == event.payload
                {
                    return Ok(true);
                }
                return Err(StoreError::new(format!(
                    "chat transcript event '{}' already exists with different content",
                    event.event_id
                )));
            }
            let current_sequence = events.last().map(|event| event.sequence).unwrap_or(0);
            if current_sequence != expected_sequence
                || event.sequence != expected_sequence.saturating_add(1)
            {
                return Ok(false);
            }
            let step = chat_step(&event, None);
            let record_json = encode_record(&step)?;
            sqlx::query(
                r#"
                INSERT INTO agent_steps(step_id, thread_id, created_at_sort, record_json)
                VALUES (?, ?, ?, ?)
                "#,
            )
            .bind(&step.step_id.0)
            .bind(&step.thread_id.0)
            .bind(sort_key(step.created_at)?)
            .bind(record_json)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlx_err)?;
            Ok(true)
        }
        .await;
        match result {
            Ok(value) => {
                sqlx::query("COMMIT")
                    .execute(&mut *connection)
                    .await
                    .map_err(map_sqlx_err)?;
                Ok(value)
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                Err(error)
            }
        }
    }

    async fn list_chat_transcript_events_after(
        &self,
        thread_id: &ThreadId,
        after: u64,
    ) -> Result<Vec<ChatTranscriptEvent>, StoreError> {
        let mut connection = self.pool.acquire().await.map_err(map_sqlx_err)?;
        let mut events = read_chat_steps(&mut connection, thread_id)
            .await?
            .iter()
            .filter_map(chat_event_from_step)
            .filter(|event| event.sequence > after)
            .collect::<Vec<_>>();
        events.sort_by_key(|event| event.sequence);
        Ok(events)
    }

    async fn get_context_checkpoint(
        &self,
        thread_id: &ThreadId,
    ) -> Result<Option<ContextCheckpoint>, StoreError> {
        let mut connection = self.pool.acquire().await.map_err(map_sqlx_err)?;
        let mut checkpoints = read_chat_steps(&mut connection, thread_id)
            .await?
            .iter()
            .filter_map(checkpoint_from_step)
            .collect::<Vec<_>>();
        checkpoints.sort_by_key(|checkpoint| checkpoint.archive_through_seq);
        Ok(checkpoints.pop())
    }

    async fn commit_context_checkpoint(
        &self,
        checkpoint: ContextCheckpoint,
        expected_sequence: u64,
    ) -> Result<Option<ContextCheckpointCommit>, StoreError> {
        let mut connection = self.pool.acquire().await.map_err(map_sqlx_err)?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .map_err(map_sqlx_err)?;
        let result = async {
            let steps = read_chat_steps(&mut connection, &checkpoint.thread_id).await?;
            let mut events = steps
                .iter()
                .filter_map(chat_event_from_step)
                .collect::<Vec<_>>();
            events.sort_by_key(|event| event.sequence);
            let current_checkpoint = steps
                .iter()
                .filter_map(checkpoint_from_step)
                .max_by_key(|checkpoint| checkpoint.archive_through_seq);
            if let Some(existing) = &current_checkpoint {
                if existing.checkpoint_id == checkpoint.checkpoint_id {
                    let event = events
                        .iter()
                        .find(|event| {
                            event.kind == ChatTranscriptEventKind::ContextCheckpointed
                                && event.event_id
                                    == format!("checkpoint:{}", checkpoint.checkpoint_id)
                        })
                        .cloned()
                        .ok_or_else(|| {
                            StoreError::new("checkpoint exists without checkpoint event")
                        })?;
                    return Ok(Some(ContextCheckpointCommit {
                        checkpoint: existing.clone(),
                        event,
                    }));
                }
            }
            let current_sequence = events.last().map(|event| event.sequence).unwrap_or(0);
            if current_sequence != expected_sequence {
                return Ok(None);
            }
            let current_checkpoint_id = current_checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.checkpoint_id.clone());
            if checkpoint.previous_checkpoint_id != current_checkpoint_id {
                return Ok(None);
            }
            let event = ChatTranscriptEvent {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                event_id: format!("checkpoint:{}", checkpoint.checkpoint_id),
                thread_id: checkpoint.thread_id.clone(),
                sequence: expected_sequence.saturating_add(1),
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
            let step = chat_step(&event, Some(&checkpoint));
            let record_json = encode_record(&step)?;
            sqlx::query(
                r#"
                INSERT INTO agent_steps(step_id, thread_id, created_at_sort, record_json)
                VALUES (?, ?, ?, ?)
                "#,
            )
            .bind(&step.step_id.0)
            .bind(&step.thread_id.0)
            .bind(sort_key(step.created_at)?)
            .bind(record_json)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlx_err)?;
            Ok(Some(ContextCheckpointCommit { checkpoint, event }))
        }
        .await;
        match result {
            Ok(value) => {
                sqlx::query("COMMIT")
                    .execute(&mut *connection)
                    .await
                    .map_err(map_sqlx_err)?;
                Ok(value)
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                Err(error)
            }
        }
    }
}
