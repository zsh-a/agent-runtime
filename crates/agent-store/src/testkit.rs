use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use agent_core::{
    AgentLockStore, AgentProposalStore, AgentRunEventStore, AgentRunRecord, AgentRunStatus,
    AgentRunStore, AgentSessionStore, AgentStateStore, AgentTrace, AgentTraceStore,
    PROTOCOL_VERSION, ProposalEnvelope, ProposalId, ProposalStatus, RunId, RunScope, SessionRecord,
    StepRecord, ThreadRecord, TraceEvent,
};
use serde_json::json;
use time::OffsetDateTime;

static CONFORMANCE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Assert the shared behavior expected from an `AgentRunStore` implementation.
///
/// The helper writes uniquely named records and does not require a completely
/// empty store, but implementations should make newly written records
/// immediately visible to subsequent reads in the same test.
pub async fn assert_run_store_conformance(store: &dyn AgentRunStore) {
    let prefix = unique_prefix("run");
    let agent_id = id(&prefix, "echo");
    let other_agent_id = id(&prefix, "other");
    let user_id = id(&prefix, "user_1");
    let missing_tenant_id = id(&prefix, "tenant_missing");
    let now = OffsetDateTime::now_utc();
    let older = run_record(
        &id(&prefix, "old"),
        &agent_id,
        RunScope::Global,
        now + time::Duration::seconds(1),
    );
    let other_agent = run_record(
        &id(&prefix, "other_agent"),
        &other_agent_id,
        RunScope::Global,
        now + time::Duration::seconds(2),
    );
    let newer = run_record(
        &id(&prefix, "new"),
        &agent_id,
        RunScope::Global,
        now + time::Duration::seconds(3),
    );
    let scoped = run_record(
        &id(&prefix, "user"),
        &agent_id,
        RunScope::User(user_id.clone()),
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
        .list_runs(Some(&agent_id), Some(1))
        .await
        .expect("runs listed");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].run_id, scoped.run_id);

    let global_last = store
        .last_run(&agent_id, &RunScope::Global)
        .await
        .expect("last global run reads")
        .expect("global run exists");
    assert_eq!(global_last.run_id, newer.run_id);

    let user_last = store
        .last_run(&agent_id, &RunScope::User(user_id))
        .await
        .expect("last user run reads")
        .expect("user run exists");
    assert_eq!(user_last.run_id, scoped.run_id);

    let missing_scope = store
        .last_run(&agent_id, &RunScope::Tenant(missing_tenant_id))
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

    let failed_runs = store
        .list_runs_by_status(AgentRunStatus::Failed, None)
        .await
        .expect("failed runs listed");
    assert!(
        failed_runs.iter().any(|run| run.run_id == updated.run_id),
        "status-filtered run list includes updated failed run"
    );
    let completed_runs = store
        .list_runs_by_status(AgentRunStatus::Completed, None)
        .await
        .expect("completed runs listed");
    assert!(
        completed_runs
            .iter()
            .all(|run| run.run_id != updated.run_id),
        "status-filtered run list excludes records after status updates"
    );
}

/// Assert the shared behavior expected from an `AgentRunEventStore`
/// implementation.
pub async fn assert_run_event_store_conformance(store: &dyn AgentRunEventStore) {
    let prefix = unique_prefix("run_events");
    let run_id = RunId(id(&prefix, "run"));

    assert!(
        store
            .list_run_events_after(&run_id, 0)
            .await
            .expect("missing event log checks")
            .is_none()
    );

    store
        .append_run_event(&run_id, TraceEvent::new("run_started", json!({"idx": 1})))
        .await
        .expect("first event appended");
    store
        .append_run_event(
            &run_id,
            TraceEvent::new("tool_call_finished", json!({"idx": 2})),
        )
        .await
        .expect("second event appended");
    let appended = store
        .list_run_events_after(&run_id, 0)
        .await
        .expect("appended events read")
        .expect("event log exists");
    assert_eq!(appended.len(), 2);
    assert_eq!(appended[0].cursor, 1);
    assert_eq!(appended[0].event.kind, "run_started");
    assert_eq!(appended[1].cursor, 2);
    assert_eq!(appended[1].event.kind, "tool_call_finished");

    let after_first = store
        .list_run_events_after(&run_id, 1)
        .await
        .expect("events after cursor read")
        .expect("event log exists");
    assert_eq!(after_first.len(), 1);
    assert_eq!(after_first[0].cursor, 2);

    store
        .replace_run_events(
            &run_id,
            vec![
                TraceEvent::new("run_started", json!({"replacement": true})),
                TraceEvent::new("run_finished", json!({"status": "completed"})),
            ],
        )
        .await
        .expect("events replaced");
    let replaced = store
        .list_run_events_after(&run_id, 0)
        .await
        .expect("replaced events read")
        .expect("event log exists");
    assert_eq!(replaced.len(), 2);
    assert_eq!(replaced[0].cursor, 1);
    assert_eq!(replaced[0].event.payload["replacement"], true);
    assert_eq!(replaced[1].cursor, 2);
    assert_eq!(replaced[1].event.kind, "run_finished");

    store
        .replace_run_events(&run_id, Vec::new())
        .await
        .expect("events replaced with empty log");
    let empty = store
        .list_run_events_after(&run_id, 0)
        .await
        .expect("empty event log reads")
        .expect("event log marker exists");
    assert!(empty.is_empty());
}

/// Assert the shared behavior expected from an `AgentTraceStore`
/// implementation.
pub async fn assert_trace_store_conformance(store: &dyn AgentTraceStore) {
    let prefix = unique_prefix("trace");
    let run_id = RunId(id(&prefix, "run"));

    assert!(
        store
            .read_trace(&run_id)
            .await
            .expect("missing trace checks")
            .is_none()
    );

    let mut trace = trace_record(run_id.clone(), &id(&prefix, "agent"));
    trace.events = vec![TraceEvent::new("run_started", json!({"attempt": 1}))];
    store
        .write_trace(trace.clone())
        .await
        .expect("trace written");
    let stored = store
        .read_trace(&run_id)
        .await
        .expect("trace reads")
        .expect("trace exists");
    assert_eq!(stored.run_id, run_id);
    assert_eq!(stored.events.len(), 1);
    assert_eq!(stored.events[0].payload["attempt"], 1);

    trace.events = vec![TraceEvent::new("run_finished", json!({"attempt": 2}))];
    store
        .write_trace(trace)
        .await
        .expect("trace replacement written");
    let replaced = store
        .read_trace(&run_id)
        .await
        .expect("replaced trace reads")
        .expect("trace exists");
    assert_eq!(replaced.events.len(), 1);
    assert_eq!(replaced.events[0].kind, "run_finished");
    assert_eq!(replaced.events[0].payload["attempt"], 2);
}

/// Assert the shared behavior expected from an `AgentProposalStore`
/// implementation.
pub async fn assert_proposal_store_conformance(store: &dyn AgentProposalStore) {
    let prefix = unique_prefix("proposal");
    let run_a = RunId(id(&prefix, "run_a"));
    let run_b = RunId(id(&prefix, "run_b"));
    let agent_id = id(&prefix, "echo");
    let now = OffsetDateTime::now_utc();
    let mut first = ProposalEnvelope::new(
        run_a.clone(),
        agent_id.clone(),
        "fake",
        "First proposal",
        json!({"idx": 1}),
    );
    first.created_at = now + time::Duration::seconds(1);
    let mut second = ProposalEnvelope::new(
        run_a.clone(),
        agent_id.clone(),
        "fake",
        "Second proposal",
        json!({"idx": 2}),
    );
    second.created_at = now + time::Duration::seconds(2);
    let mut other = ProposalEnvelope::new(
        run_b,
        agent_id,
        "fake",
        "Other run proposal",
        json!({"idx": 3}),
    );
    other.created_at = now + time::Duration::seconds(3);

    for proposal in [first.clone(), second.clone(), other.clone()] {
        store
            .create_proposal(proposal)
            .await
            .expect("proposal saved");
    }

    let scoped = store
        .list_proposals(Some(&run_a))
        .await
        .expect("proposals listed by run");
    assert_eq!(scoped.len(), 2);
    assert_eq!(scoped[0].proposal_id, first.proposal_id);
    assert_eq!(scoped[1].proposal_id, second.proposal_id);

    let inserted_ids = [
        first.proposal_id.clone(),
        second.proposal_id.clone(),
        other.proposal_id.clone(),
    ];
    let all = store
        .list_proposals(None)
        .await
        .expect("all proposals listed");
    let inserted = all
        .into_iter()
        .filter(|proposal| has_proposal_id(&inserted_ids, &proposal.proposal_id))
        .collect::<Vec<_>>();
    assert_eq!(inserted.len(), 3);
    assert_eq!(inserted[0].proposal_id, first.proposal_id);
    assert_eq!(inserted[1].proposal_id, second.proposal_id);
    assert_eq!(inserted[2].proposal_id, other.proposal_id);

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

/// Assert the shared behavior expected from an `AgentSessionStore`
/// implementation.
pub async fn assert_session_store_conformance(store: &dyn AgentSessionStore) {
    let prefix = unique_prefix("session");
    let session = SessionRecord::new(id(&prefix, "Debug session"), json!({"source": "test"}));
    let older_session = SessionRecord::new(id(&prefix, "Older session"), json!({}));
    let now = OffsetDateTime::now_utc();
    let mut thread = ThreadRecord::root(
        session.session_id.clone(),
        Some(id(&prefix, "Baseline")),
        json!({}),
    );
    thread.created_at = now + time::Duration::seconds(1);
    let mut child_thread = ThreadRecord::fork(
        session.session_id.clone(),
        thread.thread_id.clone(),
        Some(id(&prefix, "Fork")),
        json!({"branch": true}),
    );
    child_thread.created_at = now + time::Duration::seconds(2);
    let mut step = StepRecord::agent_run(
        thread.thread_id.clone(),
        RunId(id(&prefix, "run_test")),
        Some("completed".to_owned()),
        json!({"agent_id": id(&prefix, "echo"), "status": "completed"}),
    );
    step.created_at = now + time::Duration::seconds(3);

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
        session.title
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

/// Assert the shared behavior expected from an `AgentStateStore`
/// implementation.
pub async fn assert_state_store_conformance(store: &dyn AgentStateStore) {
    let prefix = unique_prefix("state");
    let agent_a = id(&prefix, "agent_a");
    let agent_b = id(&prefix, "agent_b");
    let key = id(&prefix, "shared_key");
    let missing_key = id(&prefix, "missing_key");

    assert!(
        store
            .load(&agent_a, &missing_key)
            .await
            .expect("missing state reads")
            .is_none()
    );

    store
        .save(&agent_a, &key, json!({"value": 1}))
        .await
        .expect("state saved");
    store
        .save(&agent_b, &key, json!({"value": 2}))
        .await
        .expect("isolated agent state saved");
    store
        .save(&agent_a, &key, json!({"value": 3}))
        .await
        .expect("state overwritten");

    assert_eq!(
        store
            .load(&agent_a, &key)
            .await
            .expect("state reads")
            .expect("state exists"),
        json!({"value": 3})
    );
    assert_eq!(
        store
            .load(&agent_b, &key)
            .await
            .expect("isolated state reads")
            .expect("isolated state exists"),
        json!({"value": 2})
    );
}

/// Assert the shared behavior expected from an `AgentLockStore`
/// implementation.
pub async fn assert_lock_store_conformance(store: &dyn AgentLockStore) {
    let prefix = unique_prefix("lock");
    let key = id(&prefix, "key");
    let owner_a = id(&prefix, "owner_a");
    let owner_b = id(&prefix, "owner_b");
    let ttl = std::time::Duration::from_secs(60);

    let first = store
        .acquire(&key, &owner_a, ttl)
        .await
        .expect("first lock acquisition checks")
        .expect("first owner gets lease");
    assert_eq!(first.owner, owner_a);

    let contended = store
        .acquire(&key, &owner_b, ttl)
        .await
        .expect("contended lock checks");
    assert!(contended.is_none());

    let replaced = store
        .acquire(&key, &owner_a, ttl)
        .await
        .expect("same owner reacquires")
        .expect("same owner gets replacement lease");
    assert_eq!(replaced.owner, owner_a);
    assert!(replaced.expires_at >= first.expires_at);

    store
        .renew(
            &agent_core::RunLease {
                owner: owner_b.clone(),
                ..replaced.clone()
            },
            ttl,
        )
        .await
        .expect("wrong-owner renew is ignored");
    let still_contended = store
        .acquire(&key, &owner_b, ttl)
        .await
        .expect("renewed lock checks");
    assert!(still_contended.is_none());

    store
        .release(agent_core::RunLease {
            owner: owner_b.clone(),
            ..replaced.clone()
        })
        .await
        .expect("wrong-owner release is ignored");
    let still_held = store
        .acquire(&key, &owner_b, ttl)
        .await
        .expect("wrong-owner release checks");
    assert!(still_held.is_none());

    store.release(replaced).await.expect("lock released");
    let second = store
        .acquire(&key, &owner_b, ttl)
        .await
        .expect("second lock acquired")
        .expect("second owner gets released lease");
    assert_eq!(second.owner, owner_b);
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

fn trace_record(run_id: RunId, agent_id: &str) -> AgentTrace {
    let now = OffsetDateTime::now_utc();
    AgentTrace {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        runtime_version: "test-runtime".to_owned(),
        run_id,
        agent_id: agent_id.to_owned(),
        agent_version: "test-agent".to_owned(),
        scope: RunScope::Global,
        started_at: now,
        finished_at: now,
        input: json!({}),
        output: json!({}),
        workflow: None,
        usage_summary: None,
        spans: Vec::new(),
        events: Vec::new(),
        artifact_refs: Vec::new(),
    }
}

fn unique_prefix(kind: &str) -> String {
    let counter = CONFORMANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!(
        "store_conformance_{kind}_{}_{}_{}",
        std::process::id(),
        nanos,
        counter
    )
}

fn id(prefix: &str, suffix: &str) -> String {
    format!("{prefix}_{suffix}")
}

fn has_proposal_id(proposal_ids: &[ProposalId], proposal_id: &ProposalId) -> bool {
    proposal_ids
        .iter()
        .any(|candidate| candidate == proposal_id)
}
