use super::*;

impl RuntimeServer {
    pub(crate) async fn create_session(
        &self,
        params: HttpSessionCreateParams,
    ) -> Result<HttpSessionCreateResponse> {
        info!(title = %params.title, "server create_session requested");
        if params.title.trim().is_empty() {
            return Err(miette!("session title cannot be empty"));
        }
        let session = SessionRecord::new(params.title.clone(), params.metadata);
        let thread = ThreadRecord::root(
            session.session_id.clone(),
            Some(params.title),
            json!({"source": "http"}),
        );
        self.session_store
            .create_session(session.clone())
            .await
            .into_diagnostic()?;
        self.session_store
            .create_thread(thread.clone())
            .await
            .into_diagnostic()?;
        Ok(HttpSessionCreateResponse { session, thread })
    }

    pub(crate) async fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        self.session_store.list_sessions().await.into_diagnostic()
    }

    pub(crate) async fn show_session(&self, session_id: SessionId) -> Result<SessionShowReport> {
        let session = self
            .session_store
            .get_session(&session_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("session '{}' was not found", session_id.0))?;
        let mut threads = Vec::new();
        for thread in self
            .session_store
            .list_threads(&session.session_id)
            .await
            .into_diagnostic()?
        {
            let steps = self
                .session_store
                .list_steps(&thread.thread_id)
                .await
                .into_diagnostic()?;
            threads.push(ThreadWithSteps { thread, steps });
        }
        Ok(SessionShowReport { session, threads })
    }

    pub(crate) async fn fork_thread(
        &self,
        session_id: SessionId,
        params: HttpThreadForkParams,
    ) -> Result<ThreadForkReport> {
        info!(
            session_id = %session_id.0,
            parent_thread_id = %params.parent_thread_id,
            "server fork_thread requested",
        );
        self.session_store
            .get_session(&session_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("session '{}' was not found", session_id.0))?;
        let parent_thread_id = ThreadId(params.parent_thread_id);
        let parent = self
            .session_store
            .get_thread(&parent_thread_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("thread '{}' was not found", parent_thread_id.0))?;
        if parent.session_id != session_id {
            return Err(miette!(
                "thread '{}' does not belong to session '{}'",
                parent_thread_id.0,
                session_id.0
            ));
        }
        let thread = ThreadRecord::fork(
            session_id.clone(),
            parent_thread_id.clone(),
            params.title,
            params.metadata,
        );
        self.session_store
            .create_thread(thread.clone())
            .await
            .into_diagnostic()?;
        Ok(ThreadForkReport {
            session_id: session_id.0,
            parent_thread_id: parent_thread_id.0,
            thread,
        })
    }
}
