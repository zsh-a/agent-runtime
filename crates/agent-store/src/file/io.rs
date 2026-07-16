use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::io::AsyncWriteExt;

use super::*;

static TEMP_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) async fn write_json(
    path: &Utf8Path,
    value: &impl serde::Serialize,
) -> Result<(), StoreError> {
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

pub(super) async fn create_json(
    path: &Utf8Path,
    value: &impl serde::Serialize,
) -> Result<(), StoreError> {
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

pub(super) async fn write_json_lines(
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

pub(super) async fn append_json_line(
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

pub(super) async fn read_optional_json<T>(path: &Utf8Path) -> Result<Option<T>, StoreError>
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

pub(super) async fn read_json_records<T>(dir: &Utf8Path) -> Result<Vec<T>, StoreError>
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

pub(super) async fn read_json_line_records_after(
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

pub(super) fn run_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("runs")
}

pub(super) fn proposal_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("proposals")
}

pub(super) fn session_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("sessions")
}

pub(super) fn thread_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("threads")
}

pub(super) fn step_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("steps")
}

pub(super) fn lock_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("locks")
}

pub(super) fn trace_dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join("traces")
}

pub(super) fn trace_path(root: &Utf8Path, run_id: &RunId) -> Utf8PathBuf {
    trace_dir(root).join(format!("{}.trace.json", run_id.0))
}

pub(super) fn run_event_path(root: &Utf8Path, run_id: &RunId) -> Utf8PathBuf {
    trace_dir(root).join(format!(
        "{}.events.jsonl",
        blake3::hash(run_id.0.as_bytes()).to_hex()
    ))
}

pub(super) fn lease_duration(ttl: Duration) -> time::Duration {
    time::Duration::seconds(ttl.as_secs().max(1) as i64)
}

pub(super) fn map_store_err(err: std::io::Error) -> StoreError {
    StoreError::new(err.to_string())
}

pub(super) fn map_json_err(err: serde_json::Error) -> StoreError {
    StoreError::new(err.to_string())
}
