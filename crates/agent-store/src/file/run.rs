use super::*;

pub struct FileRunStore {
    root: Utf8PathBuf,
    updates: Mutex<()>,
}

impl FileRunStore {
    pub async fn new(root: impl Into<Utf8PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs_err::tokio::create_dir_all(run_dir(&root))
            .await
            .map_err(map_store_err)?;
        Ok(Self {
            root,
            updates: Mutex::new(()),
        })
    }

    fn path_for(&self, run_id: &RunId) -> Utf8PathBuf {
        run_dir(&self.root).join(format!("{}.json", run_id.0))
    }
}

#[async_trait]
impl AgentRunStore for FileRunStore {
    async fn create_run(&self, run: AgentRunRecord) -> Result<(), StoreError> {
        let _guard = self.updates.lock().await;
        if self
            .find_run_by_idempotency_key(
                &run.agent_id,
                &run.scope,
                run.idempotency_key.as_deref().unwrap_or(""),
            )
            .await?
            .is_some()
        {
            return Err(StoreError::new("run idempotency key already exists"));
        }
        create_json(&self.path_for(&run.run_id), &run).await
    }

    async fn update_run(
        &self,
        run: AgentRunRecord,
        expected_version: u64,
    ) -> Result<bool, StoreError> {
        if run.version != expected_version.saturating_add(1) {
            return Err(StoreError::new(
                "updated run version must increment expected version by one",
            ));
        }
        let _guard = self.updates.lock().await;
        let path = self.path_for(&run.run_id);
        if !path.exists() {
            return Err(StoreError::new("run does not exist"));
        }
        let existing: AgentRunRecord = read_optional_json(&path)
            .await?
            .ok_or_else(|| StoreError::new("run does not exist"))?;
        if existing.version != expected_version {
            return Ok(false);
        }
        write_json(&path, &run).await.map(|_| true)
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<AgentRunRecord>, StoreError> {
        read_optional_json(&self.path_for(run_id)).await
    }

    async fn find_run_by_idempotency_key(
        &self,
        agent_id: &str,
        scope: &RunScope,
        idempotency_key: &str,
    ) -> Result<Option<AgentRunRecord>, StoreError> {
        if idempotency_key.is_empty() {
            return Ok(None);
        }
        Ok(read_json_records::<AgentRunRecord>(&run_dir(&self.root))
            .await?
            .into_iter()
            .find(|run| {
                run.agent_id == agent_id
                    && same_scope(&run.scope, scope)
                    && run.idempotency_key.as_deref() == Some(idempotency_key)
            }))
    }

    async fn list_runs(
        &self,
        agent_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>, StoreError> {
        let mut runs = read_json_records::<AgentRunRecord>(&run_dir(&self.root))
            .await?
            .into_iter()
            .filter(|run| agent_id.is_none_or(|agent_id| run.agent_id == agent_id))
            .collect::<Vec<_>>();
        sort_and_limit_runs(&mut runs, limit);
        Ok(runs)
    }

    async fn last_run(
        &self,
        agent_id: &str,
        scope: &RunScope,
    ) -> Result<Option<AgentRunRecord>, StoreError> {
        let mut entries = fs_err::tokio::read_dir(run_dir(&self.root))
            .await
            .map_err(map_store_err)?;
        let mut runs = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(map_store_err)? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs_err::tokio::read(path).await.map_err(map_store_err)?;
            let run: AgentRunRecord = serde_json::from_slice(&bytes).map_err(map_json_err)?;
            if run.agent_id == agent_id && same_scope(&run.scope, scope) {
                runs.push(run);
            }
        }
        runs.sort_by_key(|run| run.started_at);
        Ok(runs.pop())
    }
}
