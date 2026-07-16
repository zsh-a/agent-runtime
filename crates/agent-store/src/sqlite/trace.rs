use super::*;

#[async_trait]
impl AgentTraceStore for SqliteStore {
    async fn write_trace(&self, trace: AgentTrace) -> Result<(), StoreError> {
        let record_json = encode_record(&trace)?;
        sqlx::query(
            r#"
            INSERT INTO agent_traces(run_id, record_json)
            VALUES (?, ?)
            ON CONFLICT(run_id) DO UPDATE SET
                record_json = excluded.record_json
            "#,
        )
        .bind(&trace.run_id.0)
        .bind(record_json)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn read_trace(&self, run_id: &RunId) -> Result<Option<AgentTrace>, StoreError> {
        let row = sqlx::query("SELECT record_json FROM agent_traces WHERE run_id = ?")
            .bind(&run_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }
}
