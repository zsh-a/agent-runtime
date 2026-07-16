use super::*;

pub struct FileProposalStore {
    root: Utf8PathBuf,
    updates: Mutex<()>,
}

impl FileProposalStore {
    pub async fn new(root: impl Into<Utf8PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs_err::tokio::create_dir_all(proposal_dir(&root))
            .await
            .map_err(map_store_err)?;
        Ok(Self {
            root,
            updates: Mutex::new(()),
        })
    }

    fn path_for(&self, proposal_id: &ProposalId) -> Utf8PathBuf {
        proposal_dir(&self.root).join(format!("{}.json", proposal_id.0))
    }
}

#[async_trait]
impl AgentProposalStore for FileProposalStore {
    async fn create_proposal(&self, proposal: ProposalEnvelope) -> Result<(), StoreError> {
        create_json(&self.path_for(&proposal.proposal_id), &proposal).await
    }

    async fn update_proposal(
        &self,
        proposal: ProposalEnvelope,
        expected_version: u64,
    ) -> Result<bool, StoreError> {
        let _guard = self.updates.lock().await;
        let path = self.path_for(&proposal.proposal_id);
        let Some(current) = read_optional_json::<ProposalEnvelope>(&path).await? else {
            return Ok(false);
        };
        if current.version != expected_version || proposal.version != expected_version + 1 {
            return Ok(false);
        }
        write_json(&path, &proposal).await?;
        Ok(true)
    }

    async fn get_proposal(
        &self,
        proposal_id: &ProposalId,
    ) -> Result<Option<ProposalEnvelope>, StoreError> {
        read_optional_json(&self.path_for(proposal_id)).await
    }

    async fn list_proposals(
        &self,
        run_id: Option<&RunId>,
    ) -> Result<Vec<ProposalEnvelope>, StoreError> {
        let mut entries = fs_err::tokio::read_dir(proposal_dir(&self.root))
            .await
            .map_err(map_store_err)?;
        let mut proposals = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(map_store_err)? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs_err::tokio::read(path).await.map_err(map_store_err)?;
            let proposal: ProposalEnvelope =
                serde_json::from_slice(&bytes).map_err(map_json_err)?;
            if match run_id {
                Some(run_id) => proposal.run_id == *run_id,
                None => true,
            } {
                proposals.push(proposal);
            }
        }
        proposals.sort_by_key(|proposal| proposal.created_at);
        Ok(proposals)
    }
}
