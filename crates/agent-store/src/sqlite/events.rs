use super::*;

#[async_trait]
impl AgentRunEventStore for SqliteStore {
    async fn append_run_event(&self, run_id: &RunId, event: TraceEvent) -> Result<(), StoreError> {
        let event_json = encode_record(&event)?;
        let mut transaction = self.pool.begin().await.map_err(map_sqlx_err)?;
        touch_run_event_log(&mut transaction, run_id)
            .await
            .map_err(map_sqlx_err)?;
        sqlx::query(
            r#"
            INSERT INTO agent_run_events(run_id, cursor, event_json)
            SELECT ?, COALESCE(MAX(cursor), 0) + 1, ?
            FROM agent_run_events
            WHERE run_id = ?
            "#,
        )
        .bind(&run_id.0)
        .bind(event_json)
        .bind(&run_id.0)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_err)?;
        transaction.commit().await.map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn replace_run_events(
        &self,
        run_id: &RunId,
        events: Vec<TraceEvent>,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await.map_err(map_sqlx_err)?;
        touch_run_event_log(&mut transaction, run_id)
            .await
            .map_err(map_sqlx_err)?;
        sqlx::query("DELETE FROM agent_run_events WHERE run_id = ?")
            .bind(&run_id.0)
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_err)?;
        for (index, event) in events.into_iter().enumerate() {
            let cursor = checked_cursor_index(index.saturating_add(1))?;
            let event_json = encode_record(&event)?;
            sqlx::query(
                r#"
                INSERT INTO agent_run_events(run_id, cursor, event_json)
                VALUES (?, ?, ?)
                "#,
            )
            .bind(&run_id.0)
            .bind(cursor)
            .bind(event_json)
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_err)?;
        }
        transaction.commit().await.map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn list_run_events_after(
        &self,
        run_id: &RunId,
        after: RunEventCursor,
    ) -> Result<Option<Vec<RunEventRecord>>, StoreError> {
        let exists = sqlx::query("SELECT 1 FROM agent_run_event_logs WHERE run_id = ?")
            .bind(&run_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?
            .is_some();
        if !exists {
            return Ok(None);
        }
        let rows = sqlx::query(
            r#"
            SELECT cursor, event_json
            FROM agent_run_events
            WHERE run_id = ? AND cursor > ?
            ORDER BY cursor ASC
            "#,
        )
        .bind(&run_id.0)
        .bind(checked_cursor(after)?)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        rows.into_iter()
            .map(|row| {
                Ok(RunEventRecord {
                    cursor: decode_cursor(row.get::<i64, _>("cursor"))?,
                    event: decode_record(row.get::<String, _>("event_json"))?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()
            .map(Some)
    }
}
