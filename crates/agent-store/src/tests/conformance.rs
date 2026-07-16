use super::*;

#[tokio::test]
async fn file_run_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileRunStore::new(root).await.expect("store opens");
    assert_run_store_conformance(&store).await;
}

#[tokio::test]
async fn file_run_event_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileRunEventStore::new(root).await.expect("store opens");
    assert_run_event_store_conformance(&store).await;
}

#[tokio::test]
async fn file_trace_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileTraceStore::new(root).await.expect("store opens");
    assert_trace_store_conformance(&store).await;
}

#[tokio::test]
async fn in_memory_run_store_satisfies_conformance() {
    let store = InMemoryRunStore::default();
    assert_run_store_conformance(&store).await;
}

#[tokio::test]
async fn file_proposal_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileProposalStore::new(root).await.expect("store opens");
    assert_proposal_store_conformance(&store).await;
}

#[tokio::test]
async fn in_memory_proposal_store_satisfies_conformance() {
    let store = InMemoryProposalStore::default();
    assert_proposal_store_conformance(&store).await;
}

#[tokio::test]
async fn file_session_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileSessionStore::new(root).await.expect("store opens");
    assert_session_store_conformance(&store).await;
}

#[tokio::test]
async fn in_memory_session_store_satisfies_conformance() {
    let store = InMemorySessionStore::default();
    assert_session_store_conformance(&store).await;
}

#[tokio::test]
async fn in_memory_state_store_satisfies_conformance() {
    let store = InMemoryStateStore::default();
    assert_state_store_conformance(&store).await;
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_satisfies_conformance() {
    let store = SqliteStore::in_memory().await.expect("sqlite opens");
    assert_run_store_conformance(&store).await;
    assert_run_event_store_conformance(&store).await;
    assert_trace_store_conformance(&store).await;
    assert_proposal_store_conformance(&store).await;
    assert_session_store_conformance(&store).await;
    assert_state_store_conformance(&store).await;
    assert_lock_store_conformance(&store).await;
}
