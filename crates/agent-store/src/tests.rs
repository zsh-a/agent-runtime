use agent_core::{
    AgentLockStore, AgentProposalStore, AgentRunRecord, AgentRunStatus, AgentRunStore,
    AgentSessionStore, PROTOCOL_VERSION, ProposalEnvelope, ProposalStatus, RunId, RunScope,
    SessionRecord, StepRecord, ThreadRecord,
};
use camino::Utf8PathBuf;
use serde_json::json;
use std::time::Duration;
use time::OffsetDateTime;

use super::*;

#[tokio::test]
async fn file_run_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileRunStore::new(root).await.expect("store opens");
    assert_run_store_conformance(&store).await;
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
async fn file_lock_store_coordinates_lease_owners() {
    let root = temp_root();
    let store = FileLockStore::new(root).await.expect("store opens");

    let first = store
        .acquire("agent:echo:scope:global", "run_1", Duration::from_secs(60))
        .await
        .expect("lock acquired")
        .expect("first owner gets lease");
    assert_eq!(first.owner, "run_1");

    let contended = store
        .acquire("agent:echo:scope:global", "run_2", Duration::from_secs(60))
        .await
        .expect("contended lock checks");
    assert!(contended.is_none());

    store.release(first).await.expect("lock released");
    let second = store
        .acquire("agent:echo:scope:global", "run_2", Duration::from_secs(60))
        .await
        .expect("second lock acquired")
        .expect("second owner gets released lease");
    assert_eq!(second.owner, "run_2");
}

async fn assert_run_store_conformance(store: &dyn AgentRunStore) {
    let now = OffsetDateTime::now_utc();
    let older = run_record(
        "run_store_old",
        "echo",
        RunScope::Global,
        now + time::Duration::seconds(1),
    );
    let other_agent = run_record(
        "run_store_other",
        "other",
        RunScope::Global,
        now + time::Duration::seconds(2),
    );
    let newer = run_record(
        "run_store_new",
        "echo",
        RunScope::Global,
        now + time::Duration::seconds(3),
    );
    let scoped = run_record(
        "run_store_user",
        "echo",
        RunScope::User("user_1".to_owned()),
        now + time::Duration::seconds(4),
    );
    for run in [
        older.clone(),
        other_agent.clone(),
        newer.clone(),
        scoped.clone(),
    ] {
        store.create_run(run).await.expect("run saved");
    }

    let runs = store
        .list_runs(Some("echo"), Some(1))
        .await
        .expect("runs listed");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].run_id, scoped.run_id);

    let global_last = store
        .last_run("echo", &RunScope::Global)
        .await
        .expect("last global run reads")
        .expect("global run exists");
    assert_eq!(global_last.run_id, newer.run_id);

    let user_last = store
        .last_run("echo", &RunScope::User("user_1".to_owned()))
        .await
        .expect("last user run reads")
        .expect("user run exists");
    assert_eq!(user_last.run_id, scoped.run_id);

    let missing_scope = store
        .last_run("echo", &RunScope::Tenant("tenant_missing".to_owned()))
        .await
        .expect("missing scoped run reads");
    assert!(missing_scope.is_none());

    let mut updated = newer.clone();
    updated.status = AgentRunStatus::Failed;
    updated.output = json!({"updated": true});
    store
        .update_run(updated.clone())
        .await
        .expect("run updated");
    let fetched = store
        .get_run(&updated.run_id)
        .await
        .expect("run fetched")
        .expect("run exists");
    assert_eq!(fetched.status, AgentRunStatus::Failed);
    assert_eq!(fetched.output, json!({"updated": true}));
}

async fn assert_proposal_store_conformance(store: &dyn AgentProposalStore) {
    let mut first = ProposalEnvelope::new(
        RunId("run_proposal_a".to_owned()),
        "echo",
        "fake",
        "First proposal",
        json!({"idx": 1}),
    );
    first.created_at = OffsetDateTime::now_utc() + time::Duration::seconds(1);
    let mut second = ProposalEnvelope::new(
        RunId("run_proposal_a".to_owned()),
        "echo",
        "fake",
        "Second proposal",
        json!({"idx": 2}),
    );
    second.created_at = OffsetDateTime::now_utc() + time::Duration::seconds(2);
    let other = ProposalEnvelope::new(
        RunId("run_proposal_b".to_owned()),
        "echo",
        "fake",
        "Other run proposal",
        json!({"idx": 3}),
    );

    for proposal in [first.clone(), second.clone(), other.clone()] {
        store
            .create_proposal(proposal)
            .await
            .expect("proposal saved");
    }

    let scoped = store
        .list_proposals(Some(&RunId("run_proposal_a".to_owned())))
        .await
        .expect("proposals listed by run");
    assert_eq!(scoped.len(), 2);
    assert_eq!(scoped[0].proposal_id, first.proposal_id);
    assert_eq!(scoped[1].proposal_id, second.proposal_id);

    let all = store
        .list_proposals(None)
        .await
        .expect("all proposals listed");
    assert_eq!(all.len(), 3);

    let mut updated = second.clone();
    updated.status = ProposalStatus::Approved;
    store
        .update_proposal(updated.clone())
        .await
        .expect("proposal updated");
    let fetched = store
        .get_proposal(&updated.proposal_id)
        .await
        .expect("proposal fetched")
        .expect("proposal exists");
    assert_eq!(fetched.status, ProposalStatus::Approved);
}

async fn assert_session_store_conformance(store: &dyn AgentSessionStore) {
    let session = SessionRecord::new("Debug session", json!({"source": "test"}));
    let older_session = SessionRecord::new("Older session", json!({}));
    let thread = ThreadRecord::root(
        session.session_id.clone(),
        Some("Baseline".to_owned()),
        json!({}),
    );
    let child_thread = ThreadRecord::fork(
        session.session_id.clone(),
        thread.thread_id.clone(),
        Some("Fork".to_owned()),
        json!({"branch": true}),
    );
    let step = StepRecord::agent_run(
        thread.thread_id.clone(),
        RunId("run_test".to_owned()),
        Some("completed".to_owned()),
        json!({"agent_id": "echo", "status": "completed"}),
    );

    store
        .create_session(older_session.clone())
        .await
        .expect("older session saved");
    store
        .create_session(session.clone())
        .await
        .expect("session saved");
    store
        .create_thread(thread.clone())
        .await
        .expect("thread saved");
    store
        .create_thread(child_thread.clone())
        .await
        .expect("child thread saved");
    store.create_step(step.clone()).await.expect("step saved");

    assert_eq!(
        store
            .get_session(&session.session_id)
            .await
            .expect("session read")
            .expect("session exists")
            .title,
        "Debug session"
    );
    let sessions = store.list_sessions().await.expect("sessions listed");
    assert!(
        sessions
            .iter()
            .any(|item| item.session_id == session.session_id)
    );
    assert!(
        sessions
            .iter()
            .any(|item| item.session_id == older_session.session_id)
    );

    let threads = store
        .list_threads(&session.session_id)
        .await
        .expect("threads read");
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].thread_id, thread.thread_id);
    assert_eq!(threads[1].thread_id, child_thread.thread_id);
    assert_eq!(
        store
            .get_thread(&child_thread.thread_id)
            .await
            .expect("thread reads")
            .expect("thread exists")
            .parent_thread_id,
        Some(thread.thread_id.clone())
    );
    assert_eq!(
        store
            .list_steps(&thread.thread_id)
            .await
            .expect("steps read")
            .first()
            .expect("step exists")
            .step_id,
        step.step_id
    );
}

fn run_record(
    run_id: &str,
    agent_id: &str,
    scope: RunScope,
    started_at: OffsetDateTime,
) -> AgentRunRecord {
    AgentRunRecord {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: RunId(run_id.to_owned()),
        idempotency_key: Some(format!("idem_{run_id}")),
        agent_id: agent_id.to_owned(),
        status: AgentRunStatus::Completed,
        scope,
        started_at,
        finished_at: Some(started_at),
        input: json!({}),
        output: json!({}),
        error: None,
        workflow: None,
        metadata: json!({}),
    }
}

fn temp_root() -> Utf8PathBuf {
    let temp = tempfile::tempdir().expect("tempdir");
    Utf8PathBuf::from_path_buf(temp.keep()).expect("utf8 temp path")
}
