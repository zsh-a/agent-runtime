use super::*;

#[async_trait]
impl AgentStateStore for SqliteStore {
    async fn load(
        &self,
        agent_id: &str,
        scope: &RunScope,
        key: &str,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        let (scope_type, scope_id) = encode_scope(scope);
        let row = sqlx::query(
            "SELECT value_json FROM agent_state_scoped WHERE agent_id = ? AND scope_type = ? AND scope_id = ? AND state_key = ?",
        )
        .bind(agent_id)
        .bind(scope_type)
        .bind(scope_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("value_json")))
            .transpose()
    }

    async fn save(
        &self,
        agent_id: &str,
        scope: &RunScope,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), StoreError> {
        let (scope_type, scope_id) = encode_scope(scope);
        let value_json = encode_record(&value)?;
        sqlx::query(
            r#"
            INSERT INTO agent_state_scoped(agent_id, scope_type, scope_id, state_key, value_json)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(agent_id, scope_type, scope_id, state_key) DO UPDATE SET
                value_json = excluded.value_json
            "#,
        )
        .bind(agent_id)
        .bind(scope_type)
        .bind(scope_id)
        .bind(key)
        .bind(value_json)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        Ok(())
    }
}
