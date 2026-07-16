use super::*;

#[async_trait]
impl AgentProposalStore for SqliteStore {
    async fn create_proposal(&self, proposal: ProposalEnvelope) -> Result<(), StoreError> {
        insert_proposal(&self.pool, proposal).await
    }

    async fn update_proposal(
        &self,
        proposal: ProposalEnvelope,
        expected_version: u64,
    ) -> Result<bool, StoreError> {
        update_proposal_cas(&self.pool, proposal, expected_version).await
    }

    async fn get_proposal(
        &self,
        proposal_id: &ProposalId,
    ) -> Result<Option<ProposalEnvelope>, StoreError> {
        let row = sqlx::query("SELECT record_json FROM agent_proposals WHERE proposal_id = ?")
            .bind(&proposal_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }

    async fn list_proposals(
        &self,
        run_id: Option<&RunId>,
    ) -> Result<Vec<ProposalEnvelope>, StoreError> {
        let rows = match run_id {
            Some(run_id) => {
                sqlx::query(
                    r#"
                    SELECT record_json FROM agent_proposals
                    WHERE run_id = ?
                    ORDER BY created_at_sort ASC
                    "#,
                )
                .bind(&run_id.0)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query(
                    r#"
                    SELECT record_json FROM agent_proposals
                    ORDER BY created_at_sort ASC
                    "#,
                )
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }
}
