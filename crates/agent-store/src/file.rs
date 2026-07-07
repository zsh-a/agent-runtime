use agent_core::{
    AgentLockStore, AgentProposalStore, AgentRunEventStore, AgentRunRecord, AgentRunStore,
    AgentSessionStore, AgentTrace, AgentTraceStore, ProposalEnvelope, ProposalId, RunEventCursor,
    RunEventRecord, RunId, RunLease, RunScope, SessionId, SessionRecord, StepRecord, StoreError,
    ThreadId, ThreadRecord, TraceEvent,
};
use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;

use crate::util::{same_scope, sort_and_limit_runs};

static TEMP_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct FileRunStore {
    root: Utf8PathBuf,
}

pub struct FileProposalStore {
    root: Utf8PathBuf,
}

pub struct FileSessionStore {
    root: Utf8PathBuf,
}

pub struct FileLockStore {
    root: Utf8PathBuf,
}

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

impl FileLockStore {
    pub async fn new(root: impl Into<Utf8PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs_err::tokio::create_dir_all(lock_dir(&root))
            .await
            .map_err(map_store_err)?;
        Ok(Self { root })
    }

    fn path_for(&self, key: &str) -> Utf8PathBuf {
        lock_dir(&self.root).join(format!("{}.json", blake3::hash(key.as_bytes()).to_hex()))
    }
}

#[async_trait]
impl AgentLockStore for FileLockStore {
    async fn acquire(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<RunLease>, StoreError> {
        let path = self.path_for(key);
        let now = OffsetDateTime::now_utc();
        for _ in 0..3 {
            if let Some(stored) = read_optional_json::<RunLease>(&path).await?
                && stored.expires_at > now
                && stored.owner != owner
            {
                return Ok(None);
            }
            if path.exists() {
                let _ = fs_err::tokio::remove_file(&path).await;
            }
            let lease = RunLease {
                key: key.to_owned(),
                owner: owner.to_owned(),
                acquired_at: now,
                expires_at: now + lease_duration(ttl),
            };
            match create_json(&path, &lease).await {
                Ok(()) => return Ok(Some(lease)),
                Err(err) if err.to_string().contains("File exists") => continue,
                Err(err) => return Err(err),
            }
        }
        Ok(None)
    }

    async fn renew(&self, lease: &RunLease, ttl: Duration) -> Result<(), StoreError> {
        let path = self.path_for(&lease.key);
        let Some(mut stored) = read_optional_json::<RunLease>(&path).await? else {
            return Ok(());
        };
        if stored.owner == lease.owner {
            stored.expires_at = OffsetDateTime::now_utc() + lease_duration(ttl);
            write_json(&path, &stored).await?;
        }
        Ok(())
    }

    async fn release(&self, lease: RunLease) -> Result<(), StoreError> {
        let path = self.path_for(&lease.key);
        let Some(stored) = read_optional_json::<RunLease>(&path).await? else {
            return Ok(());
        };
        if stored.owner == lease.owner && path.exists() {
            fs_err::tokio::remove_file(path)
                .await
                .map_err(map_store_err)?;
        }
        Ok(())
    }
}

impl FileRunStore {
    pub async fn new(root: impl Into<Utf8PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs_err::tokio::create_dir_all(run_dir(&root))
            .await
            .map_err(map_store_err)?;
        Ok(Self { root })
    }

    fn path_for(&self, run_id: &RunId) -> Utf8PathBuf {
        run_dir(&self.root).join(format!("{}.json", run_id.0))
    }
}

#[async_trait]
impl AgentRunStore for FileRunStore {
    async fn create_run(&self, run: AgentRunRecord) -> Result<(), StoreError> {
        write_json(&self.path_for(&run.run_id), &run).await
    }

    async fn update_run(&self, run: AgentRunRecord) -> Result<(), StoreError> {
        write_json(&self.path_for(&run.run_id), &run).await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<AgentRunRecord>, StoreError> {
        read_optional_json(&self.path_for(run_id)).await
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

impl FileProposalStore {
    pub async fn new(root: impl Into<Utf8PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs_err::tokio::create_dir_all(proposal_dir(&root))
            .await
            .map_err(map_store_err)?;
        Ok(Self { root })
    }

    fn path_for(&self, proposal_id: &ProposalId) -> Utf8PathBuf {
        proposal_dir(&self.root).join(format!("{}.json", proposal_id.0))
    }
}

#[async_trait]
impl AgentProposalStore for FileProposalStore {
    async fn create_proposal(&self, proposal: ProposalEnvelope) -> Result<(), StoreError> {
        write_json(&self.path_for(&proposal.proposal_id), &proposal).await
    }

    async fn update_proposal(&self, proposal: ProposalEnvelope) -> Result<(), StoreError> {
        write_json(&self.path_for(&proposal.proposal_id), &proposal).await
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

async fn write_json(path: &Utf8Path, value: &impl serde::Serialize) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(map_json_err)?;
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::new(format!("path has no parent: {path}")))?;
    fs_err::tokio::create_dir_all(parent)
        .await
        .map_err(map_store_err)?;
    let temp_path = temp_write_path(path)?;

    let write_result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temp_path.as_std_path())
            .await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        fs_err::tokio::rename(&temp_path, path).await?;
        Ok::<(), std::io::Error>(())
    }
    .await;

    if let Err(err) = write_result {
        let _ = fs_err::tokio::remove_file(&temp_path).await;
        return Err(map_store_err(err));
    }

    Ok(())
}

async fn create_json(path: &Utf8Path, value: &impl serde::Serialize) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(map_json_err)?;
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::new(format!("path has no parent: {path}")))?;
    fs_err::tokio::create_dir_all(parent)
        .await
        .map_err(map_store_err)?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.as_std_path())
        .await
        .map_err(map_store_err)?;
    file.write_all(&bytes).await.map_err(map_store_err)?;
    file.sync_all().await.map_err(map_store_err)
}

async fn write_json_lines(
    path: &Utf8Path,
    values: &[impl serde::Serialize],
) -> Result<(), StoreError> {
    let mut bytes = Vec::new();
    for value in values {
        bytes.extend(serde_json::to_vec(value).map_err(map_json_err)?);
        bytes.push(b'\n');
    }
    write_bytes(path, bytes).await
}

async fn write_bytes(path: &Utf8Path, bytes: Vec<u8>) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::new(format!("path has no parent: {path}")))?;
    fs_err::tokio::create_dir_all(parent)
        .await
        .map_err(map_store_err)?;
    let temp_path = temp_write_path(path)?;

    let write_result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temp_path.as_std_path())
            .await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        fs_err::tokio::rename(&temp_path, path).await?;
        Ok::<(), std::io::Error>(())
    }
    .await;

    if let Err(err) = write_result {
        let _ = fs_err::tokio::remove_file(&temp_path).await;
        return Err(map_store_err(err));
    }

    Ok(())
}

async fn append_json_line(
    path: &Utf8Path,
    value: &impl serde::Serialize,
) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::new(format!("path has no parent: {path}")))?;
    fs_err::tokio::create_dir_all(parent)
        .await
        .map_err(map_store_err)?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_std_path())
        .await
        .map_err(map_store_err)?;
    let bytes = serde_json::to_vec(value).map_err(map_json_err)?;
    file.write_all(&bytes).await.map_err(map_store_err)?;
    file.write_all(b"\n").await.map_err(map_store_err)?;
    file.flush().await.map_err(map_store_err)
}

fn temp_write_path(path: &Utf8Path) -> Result<Utf8PathBuf, StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::new(format!("path has no parent: {path}")))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| StoreError::new(format!("path has no file name: {path}")))?;
    let counter = TEMP_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    Ok(parent.join(format!(
        ".{file_name}.{}.{}.{}.tmp",
        std::process::id(),
        nanos,
        counter
    )))
}

async fn read_optional_json<T>(path: &Utf8Path) -> Result<Option<T>, StoreError>
where
    T: serde::de::DeserializeOwned,
{
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs_err::tokio::read(path).await.map_err(map_store_err)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(map_json_err)
}

async fn read_json_records<T>(dir: &Utf8Path) -> Result<Vec<T>, StoreError>
where
    T: serde::de::DeserializeOwned,
{
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut entries = fs_err::tokio::read_dir(dir).await.map_err(map_store_err)?;
    let mut records = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(map_store_err)? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs_err::tokio::read(path).await.map_err(map_store_err)?;
        records.push(serde_json::from_slice(&bytes).map_err(map_json_err)?);
    }
    Ok(records)
}

async fn read_json_line_records_after(
    path: &Utf8Path,
    after: RunEventCursor,
) -> Result<Vec<RunEventRecord>, StoreError> {
    let text = fs_err::tokio::read_to_string(path)
        .await
        .map_err(map_store_err)?;
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cursor = RunEventCursor::try_from(index.saturating_add(1))
            .map_err(|_| StoreError::new(format!("JSONL cursor overflow at {path}")))?;
        if cursor <= after {
            continue;
        }
        records.push(RunEventRecord {
            cursor,
            event: serde_json::from_str(line).map_err(map_json_err)?,
        });
    }
    Ok(records)
}

fn run_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("runs")
}

fn proposal_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("proposals")
}

fn session_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("sessions")
}

fn thread_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("threads")
}

fn step_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("steps")
}

fn lock_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("locks")
}

fn trace_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("traces")
}

fn trace_path(root: &Utf8Path, run_id: &RunId) -> Utf8PathBuf {
    trace_dir(root).join(format!("{}.trace.json", run_id.0))
}

fn run_event_path(root: &Utf8Path, run_id: &RunId) -> Utf8PathBuf {
    trace_dir(root).join(format!(
        "{}.events.jsonl",
        blake3::hash(run_id.0.as_bytes()).to_hex()
    ))
}

fn lease_duration(ttl: Duration) -> time::Duration {
    time::Duration::seconds(ttl.as_secs().max(1) as i64)
}

fn map_store_err(err: std::io::Error) -> StoreError {
    StoreError::new(err.to_string())
}

fn map_json_err(err: serde_json::Error) -> StoreError {
    StoreError::new(err.to_string())
}
