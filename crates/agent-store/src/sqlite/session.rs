use super::*;

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
            ORDER BY created_at_sort ASC
            "#,
        )
        .bind(&thread_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }
}
