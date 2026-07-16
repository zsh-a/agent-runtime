use super::*;

#[async_trait]
impl AgentLockStore for SqliteStore {
    async fn acquire(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<RunLease>, StoreError> {
        let now = OffsetDateTime::now_utc();
        let lease = RunLease {
            key: key.to_owned(),
            owner: owner.to_owned(),
            acquired_at: now,
            expires_at: now + lease_duration(ttl),
        };
        let record_json = encode_record(&lease)?;
        let row = sqlx::query(
            r#"
            INSERT INTO agent_locks(
                lock_key,
                owner,
                acquired_at_sort,
                expires_at_sort,
                record_json
            )
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(lock_key) DO UPDATE SET
                owner = excluded.owner,
                acquired_at_sort = excluded.acquired_at_sort,
                expires_at_sort = excluded.expires_at_sort,
                record_json = excluded.record_json
            WHERE agent_locks.owner = excluded.owner
                OR agent_locks.expires_at_sort <= ?
            RETURNING record_json
            "#,
        )
        .bind(key)
        .bind(owner)
        .bind(sort_key(lease.acquired_at)?)
        .bind(sort_key(lease.expires_at)?)
        .bind(record_json)
        .bind(sort_key(now)?)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }

    async fn renew(&self, lease: &RunLease, ttl: Duration) -> Result<bool, StoreError> {
        let mut renewed = lease.clone();
        renewed.expires_at = OffsetDateTime::now_utc() + lease_duration(ttl);
        let record_json = encode_record(&renewed)?;
        let result = sqlx::query(
            r#"
            UPDATE agent_locks
            SET expires_at_sort = ?, record_json = ?
            WHERE lock_key = ? AND owner = ?
            "#,
        )
        .bind(sort_key(renewed.expires_at)?)
        .bind(record_json)
        .bind(&lease.key)
        .bind(&lease.owner)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        Ok(result.rows_affected() == 1)
    }

    async fn release(&self, lease: RunLease) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM agent_locks WHERE lock_key = ? AND owner = ?")
            .bind(&lease.key)
            .bind(&lease.owner)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        Ok(())
    }
}
