use super::*;

pub struct FileSessionStore {
    root: Utf8PathBuf,
}

impl FileSessionStore {
    pub async fn new(root: impl Into<Utf8PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        for dir in [session_dir(&root), thread_dir(&root), step_dir(&root)] {
            fs_err::tokio::create_dir_all(dir)
                .await
                .map_err(map_store_err)?;
        }
        Ok(Self { root })
    }

    fn session_path_for(&self, session_id: &SessionId) -> Utf8PathBuf {
        session_dir(&self.root).join(format!("{}.json", session_id.0))
    }

    fn thread_path_for(&self, thread_id: &ThreadId) -> Utf8PathBuf {
        thread_dir(&self.root).join(format!("{}.json", thread_id.0))
    }

    fn step_path_for(&self, step: &StepRecord) -> Utf8PathBuf {
        step_dir(&self.root).join(format!("{}.json", step.step_id.0))
    }
}

#[async_trait]
impl AgentSessionStore for FileSessionStore {
    async fn create_session(&self, session: SessionRecord) -> Result<(), StoreError> {
        write_json(&self.session_path_for(&session.session_id), &session).await
    }

    async fn list_sessions(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let mut sessions = read_json_records::<SessionRecord>(&session_dir(&self.root)).await?;
        sessions.sort_by_key(|session| session.updated_at);
        sessions.reverse();
        Ok(sessions)
    }

    async fn get_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionRecord>, StoreError> {
        read_optional_json(&self.session_path_for(session_id)).await
    }

    async fn create_thread(&self, thread: ThreadRecord) -> Result<(), StoreError> {
        write_json(&self.thread_path_for(&thread.thread_id), &thread).await
    }

    async fn list_threads(&self, session_id: &SessionId) -> Result<Vec<ThreadRecord>, StoreError> {
        let mut threads = read_json_records::<ThreadRecord>(&thread_dir(&self.root))
            .await?
            .into_iter()
            .filter(|thread| thread.session_id == *session_id)
            .collect::<Vec<_>>();
        threads.sort_by_key(|thread| thread.created_at);
        Ok(threads)
    }

    async fn get_thread(&self, thread_id: &ThreadId) -> Result<Option<ThreadRecord>, StoreError> {
        read_optional_json(&self.thread_path_for(thread_id)).await
    }

    async fn create_step(&self, step: StepRecord) -> Result<(), StoreError> {
        write_json(&self.step_path_for(&step), &step).await
    }

    async fn list_steps(&self, thread_id: &ThreadId) -> Result<Vec<StepRecord>, StoreError> {
        let mut steps = read_json_records::<StepRecord>(&step_dir(&self.root))
            .await?
            .into_iter()
            .filter(|step| step.thread_id == *thread_id)
            .collect::<Vec<_>>();
        steps.sort_by_key(|step| step.created_at);
        Ok(steps)
    }
}
