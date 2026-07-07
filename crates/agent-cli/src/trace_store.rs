use agent_core::{RunId, TraceEvent};
use camino::{Utf8Path, Utf8PathBuf};
use miette::{IntoDiagnostic, Result, miette};
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

pub(crate) type RunEventCursor = u64;

#[derive(Debug, Clone)]
pub(crate) struct RunEventRecord {
    pub(crate) cursor: RunEventCursor,
    pub(crate) event: TraceEvent,
}

pub(crate) async fn write_store_trace(
    store: &Utf8Path,
    trace: &agent_core::AgentTrace,
) -> Result<()> {
    write_json(store_trace_path(store, &trace.run_id), trace).await
}

pub(crate) async fn write_workflow_traces(
    store: &Utf8Path,
    result: &agent_core::WorkflowRunResult,
) -> Result<usize> {
    let mut written = 0;
    for node in &result.nodes {
        if let Some(trace) = node.trace.as_ref() {
            write_store_trace(store, trace).await?;
            written += 1;
        }
        if let Some(trace) = node
            .compensation
            .as_ref()
            .and_then(|compensation| compensation.trace.as_ref())
        {
            write_store_trace(store, trace).await?;
            written += 1;
        }
    }
    Ok(written)
}

pub(crate) async fn read_store_trace(store: &Utf8Path, run_id: &RunId) -> Result<Option<Value>> {
    let path = store_trace_path(store, run_id);
    if !path.exists() {
        return Ok(None);
    }
    read_json(path).await.map(Some)
}

pub(crate) async fn append_store_run_event(
    store: &Utf8Path,
    run_id: &RunId,
    event: &TraceEvent,
) -> Result<()> {
    append_json_line(store_run_events_path(store, run_id), event).await
}

pub(crate) async fn write_store_run_events(
    store: &Utf8Path,
    run_id: &RunId,
    events: &[TraceEvent],
) -> Result<()> {
    let path = store_run_events_path(store, run_id);
    let bytes = encode_json_lines(events)?;
    atomic_write(path, bytes).await
}

fn encode_json_lines<T: Serialize>(values: &[T]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for value in values {
        bytes.extend(serde_json::to_vec(value).into_diagnostic()?);
        bytes.push(b'\n');
    }
    Ok(bytes)
}

async fn atomic_write(path: Utf8PathBuf, bytes: Vec<u8>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::tokio::create_dir_all(parent)
            .await
            .into_diagnostic()?;
    }
    tokio::task::spawn_blocking(move || atomic_write_blocking(path, bytes))
        .await
        .into_diagnostic()?
}

fn atomic_write_blocking(path: Utf8PathBuf, bytes: Vec<u8>) -> Result<()> {
    use std::io::Write;

    let parent = path
        .parent()
        .ok_or_else(|| miette!("cannot atomically write path without parent: {path}"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent.as_std_path()).into_diagnostic()?;
    temp.write_all(&bytes).into_diagnostic()?;
    temp.flush().into_diagnostic()?;
    temp.as_file().sync_all().into_diagnostic()?;
    temp.persist(path.as_std_path())
        .map(|_| ())
        .map_err(|err| err.error)
        .into_diagnostic()
}

#[cfg(test)]
pub(crate) async fn read_store_run_events(
    store: &Utf8Path,
    run_id: &RunId,
) -> Result<Option<Vec<TraceEvent>>> {
    read_store_run_event_records_after(store, run_id, None)
        .await
        .map(|records| {
            records.map(|records| records.into_iter().map(|record| record.event).collect())
        })
}

pub(crate) async fn read_store_run_event_records_after(
    store: &Utf8Path,
    run_id: &RunId,
    after: Option<RunEventCursor>,
) -> Result<Option<Vec<RunEventRecord>>> {
    let path = store_run_events_path(store, run_id);
    if !path.exists() {
        return Ok(None);
    }
    read_json_line_records_after(path, after.unwrap_or(0))
        .await
        .map(Some)
}

fn store_trace_path(store: &Utf8Path, run_id: &RunId) -> Utf8PathBuf {
    store
        .join("traces")
        .join(format!("{}.trace.json", run_id.0))
}

fn store_run_events_path(store: &Utf8Path, run_id: &RunId) -> Utf8PathBuf {
    store.join("traces").join(format!(
        "{}.events.jsonl",
        blake3::hash(run_id.0.as_bytes()).to_hex()
    ))
}

pub(crate) async fn read_json(path: Utf8PathBuf) -> Result<Value> {
    let bytes = fs_err::tokio::read(&path)
        .await
        .map_err(|e| miette!("failed to read JSON at {path}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| miette!("failed to parse JSON at {path}: {e}"))
}

pub(crate) async fn read_trace(path: Utf8PathBuf) -> Result<agent_core::AgentTrace> {
    let value = read_json(path.clone()).await?;
    serde_json::from_value(value).map_err(|e| miette!("failed to parse trace at {path}: {e}"))
}

pub(crate) async fn write_json(path: Utf8PathBuf, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::tokio::create_dir_all(parent)
            .await
            .into_diagnostic()?;
    }
    let bytes = serde_json::to_vec_pretty(value).into_diagnostic()?;
    fs_err::tokio::write(path, bytes).await.into_diagnostic()
}

pub(crate) async fn write_text(path: Utf8PathBuf, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::tokio::create_dir_all(parent)
            .await
            .into_diagnostic()?;
    }
    fs_err::tokio::write(path, text).await.into_diagnostic()
}

async fn append_json_line(path: Utf8PathBuf, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::tokio::create_dir_all(parent)
            .await
            .into_diagnostic()?;
    }
    let mut file = fs_err::tokio::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .into_diagnostic()?;
    let bytes = serde_json::to_vec(value).into_diagnostic()?;
    file.write_all(&bytes).await.into_diagnostic()?;
    file.write_all(b"\n").await.into_diagnostic()?;
    file.flush().await.into_diagnostic()
}

async fn read_json_line_records_after(
    path: Utf8PathBuf,
    after: RunEventCursor,
) -> Result<Vec<RunEventRecord>> {
    let text = fs_err::tokio::read_to_string(&path)
        .await
        .map_err(|e| miette!("failed to read JSONL at {path}: {e}"))?;
    let mut values = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cursor = RunEventCursor::try_from(index.saturating_add(1))
            .map_err(|_| miette!("JSONL cursor overflow at {path} line {}", index + 1))?;
        if cursor <= after {
            continue;
        }
        values.push(RunEventRecord {
            cursor,
            event: serde_json::from_str(line).map_err(|e| {
                miette!(
                    "failed to parse JSONL at {path} line {}: {e}",
                    index.saturating_add(1)
                )
            })?,
        });
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn run_event_log_round_trips_jsonl() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = Utf8PathBuf::from_path_buf(temp.path().join("store")).expect("utf8 path");
        let run_id = RunId("run_event_round_trip".to_owned());
        let events = vec![
            TraceEvent::new("run_started", json!({"agent_id": "echo"})),
            TraceEvent::new("run_finished", json!({"status": "completed"})),
        ];

        write_store_run_events(&store, &run_id, &events)
            .await
            .expect("events write");
        let read = read_store_run_events(&store, &run_id)
            .await
            .expect("events read")
            .expect("event log exists");

        assert_eq!(read.len(), 2);
        assert_eq!(read[0].kind, "run_started");
        assert_eq!(read[1].kind, "run_finished");
    }

    #[tokio::test]
    async fn run_event_log_rewrite_replaces_previous_events() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = Utf8PathBuf::from_path_buf(temp.path().join("store")).expect("utf8 path");
        let run_id = RunId("run_event_replace".to_owned());
        let old_events = vec![
            TraceEvent::new("run_started", json!({"attempt": 1})),
            TraceEvent::new("old_partial_event", json!({})),
        ];
        let new_events = vec![
            TraceEvent::new("run_started", json!({"attempt": 2})),
            TraceEvent::new("run_finished", json!({"status": "completed"})),
        ];

        write_store_run_events(&store, &run_id, &old_events)
            .await
            .expect("old events write");
        write_store_run_events(&store, &run_id, &new_events)
            .await
            .expect("new events replace old log");
        let read = read_store_run_events(&store, &run_id)
            .await
            .expect("events read")
            .expect("event log exists");

        assert_eq!(read.len(), 2);
        assert_eq!(read[0].payload["attempt"], 2);
        assert_eq!(read[1].kind, "run_finished");
        assert!(
            read.iter().all(|event| event.kind != "old_partial_event"),
            "atomic final rewrite should replace the old JSONL contents"
        );
    }

    #[tokio::test]
    async fn run_event_log_reads_records_after_cursor() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = Utf8PathBuf::from_path_buf(temp.path().join("store")).expect("utf8 path");
        let run_id = RunId("run_event_after".to_owned());
        let events = vec![
            TraceEvent::new("run_started", json!({"index": 1})),
            TraceEvent::new("tool_call_started", json!({"index": 2})),
            TraceEvent::new("run_finished", json!({"index": 3})),
        ];

        write_store_run_events(&store, &run_id, &events)
            .await
            .expect("events write");
        let read = read_store_run_event_records_after(&store, &run_id, Some(1))
            .await
            .expect("events read")
            .expect("event log exists");

        assert_eq!(read.len(), 2);
        assert_eq!(read[0].cursor, 2);
        assert_eq!(read[0].event.kind, "tool_call_started");
        assert_eq!(read[1].cursor, 3);
        assert_eq!(read[1].event.kind, "run_finished");
    }
}
