use agent_core::{SessionId, ThreadId};
use camino::Utf8PathBuf;
use clap::Subcommand;
use miette::{IntoDiagnostic, Result};

use crate::{
    config::{RuntimeStoreBackend, configured_path},
    print_json,
    runtime_stores::RuntimeStores,
    session::{create_session, fork_thread, show_session},
};

const DEFAULT_STORE: &str = ".agent-runtime/store";

#[derive(Debug, Subcommand)]
pub(crate) enum SessionCommand {
    Create {
        #[arg(long)]
        title: String,
        #[arg(long, default_value = ".agent-runtime/store")]
        store: Utf8PathBuf,
    },
    List {
        #[arg(long, default_value = ".agent-runtime/store")]
        store: Utf8PathBuf,
    },
    Show {
        session_id: String,
        #[arg(long, default_value = ".agent-runtime/store")]
        store: Utf8PathBuf,
    },
    Fork {
        session_id: String,
        parent_thread_id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, default_value = ".agent-runtime/store")]
        store: Utf8PathBuf,
    },
}

pub(crate) async fn run_session_command(
    command: SessionCommand,
    store_backend: RuntimeStoreBackend,
    configured_store: Option<Utf8PathBuf>,
) -> Result<()> {
    match command {
        SessionCommand::Create { title, store } => {
            let stores = session_stores(store, store_backend, configured_store.as_ref()).await?;
            let report = create_session(stores.session_store.as_ref(), title).await?;
            print_json(&report)
        }
        SessionCommand::List { store } => {
            let stores = session_stores(store, store_backend, configured_store.as_ref()).await?;
            let sessions = stores
                .session_store
                .list_sessions()
                .await
                .into_diagnostic()?;
            print_json(&sessions)
        }
        SessionCommand::Show { session_id, store } => {
            let stores = session_stores(store, store_backend, configured_store.as_ref()).await?;
            let report = show_session(stores.session_store.as_ref(), SessionId(session_id)).await?;
            print_json(&report)
        }
        SessionCommand::Fork {
            session_id,
            parent_thread_id,
            title,
            store,
        } => {
            let stores = session_stores(store, store_backend, configured_store.as_ref()).await?;
            let report = fork_thread(
                stores.session_store.as_ref(),
                SessionId(session_id),
                ThreadId(parent_thread_id),
                title,
            )
            .await?;
            print_json(&report)
        }
    }
}

async fn session_stores(
    store: Utf8PathBuf,
    store_backend: RuntimeStoreBackend,
    configured_store: Option<&Utf8PathBuf>,
) -> Result<RuntimeStores> {
    RuntimeStores::open(
        store_backend,
        configured_path(store, DEFAULT_STORE, configured_store),
    )
    .await
}
