use std::sync::Arc;

use agent_core::{
    AgentLockStore, AgentProposalStore, AgentRunEventStore, AgentRunStore, AgentSessionStore,
    AgentStateStore, AgentTraceStore,
};
use agent_store::{
    FileLockStore, FileProposalStore, FileRunEventStore, FileRunStore, FileSessionStore,
    FileTraceStore, InMemoryStateStore, SqliteStore,
};
use camino::{Utf8Path, Utf8PathBuf};
use miette::{IntoDiagnostic, Result};

use crate::config::RuntimeStoreBackend;

#[derive(Clone)]
pub(crate) struct RuntimeStores {
    pub(crate) artifact_store_path: Utf8PathBuf,
    pub(crate) run_store: Arc<dyn AgentRunStore>,
    pub(crate) event_store: Arc<dyn AgentRunEventStore>,
    pub(crate) trace_store: Arc<dyn AgentTraceStore>,
    pub(crate) proposal_store: Arc<dyn AgentProposalStore>,
    pub(crate) session_store: Arc<dyn AgentSessionStore>,
    pub(crate) state_store: Arc<dyn AgentStateStore>,
    pub(crate) lock_store: Arc<dyn AgentLockStore>,
}

impl RuntimeStores {
    pub(crate) async fn open(
        backend: RuntimeStoreBackend,
        artifact_store_path: Utf8PathBuf,
    ) -> Result<Self> {
        match backend {
            RuntimeStoreBackend::File => Self::open_file(artifact_store_path).await,
            RuntimeStoreBackend::Sqlite => Self::open_sqlite(artifact_store_path).await,
        }
    }

    pub(crate) fn sqlite_path(artifact_store_path: &Utf8Path) -> Utf8PathBuf {
        artifact_store_path.join("runtime.sqlite")
    }

    async fn open_file(artifact_store_path: Utf8PathBuf) -> Result<Self> {
        Ok(Self {
            run_store: Arc::new(
                FileRunStore::new(artifact_store_path.clone())
                    .await
                    .into_diagnostic()?,
            ),
            event_store: Arc::new(
                FileRunEventStore::new(artifact_store_path.clone())
                    .await
                    .into_diagnostic()?,
            ),
            trace_store: Arc::new(
                FileTraceStore::new(artifact_store_path.clone())
                    .await
                    .into_diagnostic()?,
            ),
            proposal_store: Arc::new(
                FileProposalStore::new(artifact_store_path.clone())
                    .await
                    .into_diagnostic()?,
            ),
            session_store: Arc::new(
                FileSessionStore::new(artifact_store_path.clone())
                    .await
                    .into_diagnostic()?,
            ),
            state_store: Arc::new(InMemoryStateStore::default()),
            lock_store: Arc::new(
                FileLockStore::new(artifact_store_path.clone())
                    .await
                    .into_diagnostic()?,
            ),
            artifact_store_path,
        })
    }

    async fn open_sqlite(artifact_store_path: Utf8PathBuf) -> Result<Self> {
        let sqlite = Arc::new(
            SqliteStore::open(Self::sqlite_path(&artifact_store_path))
                .await
                .into_diagnostic()?,
        );
        let run_store: Arc<dyn AgentRunStore> = sqlite.clone();
        let event_store: Arc<dyn AgentRunEventStore> = sqlite.clone();
        let trace_store: Arc<dyn AgentTraceStore> = Arc::new(
            FileTraceStore::new(artifact_store_path.clone())
                .await
                .into_diagnostic()?,
        );
        let proposal_store: Arc<dyn AgentProposalStore> = sqlite.clone();
        let session_store: Arc<dyn AgentSessionStore> = sqlite.clone();
        let state_store: Arc<dyn AgentStateStore> = sqlite.clone();
        let lock_store: Arc<dyn AgentLockStore> = sqlite;
        Ok(Self {
            artifact_store_path,
            run_store,
            event_store,
            trace_store,
            proposal_store,
            session_store,
            state_store,
            lock_store,
        })
    }
}
