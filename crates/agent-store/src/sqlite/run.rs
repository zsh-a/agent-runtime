use super::*;

#[async_trait]
impl AgentRunStore for SqliteStore {
    async fn create_run(&self, run: AgentRunRecord) -> Result<(), StoreError> {
        insert_run(&self.pool, run).await
    }

    async fn update_run(
        &self,
        run: AgentRunRecord,
        expected_version: u64,
    ) -> Result<bool, StoreError> {
        update_run_record(&self.pool, run, expected_version).await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<AgentRunRecord>, StoreError> {
        let row = sqlx::query("SELECT record_json FROM agent_runs WHERE run_id = ?")
            .bind(&run_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }

    async fn find_run_by_idempotency_key(
        &self,
        agent_id: &str,
        scope: &RunScope,
        idempotency_key: &str,
    ) -> Result<Option<AgentRunRecord>, StoreError> {
        let (scope_type, scope_id) = encode_scope(scope);
        let row = sqlx::query(
            r#"
            SELECT record_json FROM agent_runs
            WHERE agent_id = ? AND scope_type = ? AND scope_id = ?
              AND json_extract(record_json, '$.idempotency_key') = ?
            LIMIT 1
            "#,
        )
        .bind(agent_id)
        .bind(scope_type)
        .bind(scope_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }

    async fn list_runs(
        &self,
        agent_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>, StoreError> {
        let limit = limit.unwrap_or(i64::MAX as usize);
        let rows = match agent_id {
            Some(agent_id) => {
                sqlx::query(
                    r#"
                    SELECT record_json FROM agent_runs
                    WHERE agent_id = ?
                    ORDER BY started_at_sort DESC
                    LIMIT ?
                    "#,
                )
                .bind(agent_id)
                .bind(checked_limit(limit)?)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query(
                    r#"
                    SELECT record_json FROM agent_runs
                    ORDER BY started_at_sort DESC
                    LIMIT ?
                    "#,
                )
                .bind(checked_limit(limit)?)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }

    async fn list_runs_by_status(
        &self,
        status: AgentRunStatus,
        limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT record_json FROM agent_runs
            WHERE status = ?
            ORDER BY started_at_sort DESC
            LIMIT ?
            "#,
        )
        .bind(encode_run_status(&status))
        .bind(checked_limit(limit.unwrap_or(i64::MAX as usize))?)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }

    async fn last_run(
        &self,
        agent_id: &str,
        scope: &RunScope,
    ) -> Result<Option<AgentRunRecord>, StoreError> {
        let (scope_type, scope_id) = encode_scope(scope);
        let row = sqlx::query(
            r#"
            SELECT record_json FROM agent_runs
            WHERE agent_id = ? AND scope_type = ? AND scope_id = ?
            ORDER BY started_at_sort DESC
            LIMIT 1
            "#,
        )
        .bind(agent_id)
        .bind(scope_type)
        .bind(scope_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }
}

async fn insert_run(pool: &SqlitePool, run: AgentRunRecord) -> Result<(), StoreError> {
    let (scope_type, scope_id) = encode_scope(&run.scope);
    let status = encode_run_status(&run.status);
    let record_json = encode_record(&run)?;
    sqlx::query(
        r#"
        INSERT INTO agent_runs(
            run_id,
            agent_id,
            scope_type,
            scope_id,
            started_at_sort,
            status,
            record_json
        )
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&run.run_id.0)
    .bind(&run.agent_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(sort_key(run.started_at)?)
    .bind(status)
    .bind(record_json)
    .execute(pool)
    .await
    .map_err(map_sqlx_err)?;
    Ok(())
}

async fn update_run_record(
    pool: &SqlitePool,
    run: AgentRunRecord,
    expected_version: u64,
) -> Result<bool, StoreError> {
    if run.version != expected_version.saturating_add(1) {
        return Err(StoreError::new(
            "updated run version must increment expected version by one",
        ));
    }
    let (scope_type, scope_id) = encode_scope(&run.scope);
    let record_json = encode_record(&run)?;
    let result = sqlx::query(
        r#"
        UPDATE agent_runs
        SET agent_id = ?, scope_type = ?, scope_id = ?, started_at_sort = ?, status = ?, record_json = ?
        WHERE run_id = ?
          AND COALESCE(json_extract(record_json, '$.version'), 1) = ?
        "#,
    )
    .bind(&run.agent_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(sort_key(run.started_at)?)
    .bind(encode_run_status(&run.status))
    .bind(record_json)
    .bind(&run.run_id.0)
    .bind(checked_record_version(expected_version)?)
    .execute(pool)
    .await
    .map_err(map_sqlx_err)?;
    if result.rows_affected() == 0 {
        let exists = sqlx::query("SELECT 1 FROM agent_runs WHERE run_id = ?")
            .bind(&run.run_id.0)
            .fetch_optional(pool)
            .await
            .map_err(map_sqlx_err)?
            .is_some();
        if exists {
            return Ok(false);
        }
        return Err(StoreError::new("run does not exist"));
    }
    Ok(true)
}
