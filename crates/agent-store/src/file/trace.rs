use super::*;

pub struct FileRunEventStore {
    root: Utf8PathBuf,
}

pub struct FileTraceStore {
    root: Utf8PathBuf,
}

impl FileTraceStore {
    pub async fn new(root: impl Into<Utf8PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs_err::tokio::create_dir_all(trace_dir(&root))
            .await
            .map_err(map_store_err)?;
        Ok(Self { root })
    }

    fn path_for(&self, run_id: &RunId) -> Utf8PathBuf {
        trace_path(&self.root, run_id)
    }
}

#[async_trait]
impl AgentTraceStore for FileTraceStore {
    async fn write_trace(&self, trace: AgentTrace) -> Result<(), StoreError> {
        write_json(&self.path_for(&trace.run_id), &trace).await
    }

    async fn read_trace(&self, run_id: &RunId) -> Result<Option<AgentTrace>, StoreError> {
        read_optional_json(&self.path_for(run_id)).await
    }
}

impl FileRunEventStore {
    pub async fn new(root: impl Into<Utf8PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs_err::tokio::create_dir_all(trace_dir(&root))
            .await
            .map_err(map_store_err)?;
        Ok(Self { root })
    }

    fn path_for(&self, run_id: &RunId) -> Utf8PathBuf {
        run_event_path(&self.root, run_id)
    }
}

#[async_trait]
impl AgentRunEventStore for FileRunEventStore {
    async fn append_run_event(&self, run_id: &RunId, event: TraceEvent) -> Result<(), StoreError> {
        append_json_line(&self.path_for(run_id), &event).await
    }

    async fn replace_run_events(
        &self,
        run_id: &RunId,
        events: Vec<TraceEvent>,
    ) -> Result<(), StoreError> {
        write_json_lines(&self.path_for(run_id), &events).await
    }

    async fn list_run_events_after(
        &self,
        run_id: &RunId,
        after: RunEventCursor,
    ) -> Result<Option<Vec<RunEventRecord>>, StoreError> {
        let path = self.path_for(run_id);
        if !path.exists() {
            return Ok(None);
        }
        read_json_line_records_after(&path, after).await.map(Some)
    }
}
