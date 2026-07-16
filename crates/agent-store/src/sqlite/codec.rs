use std::time::Duration;

use agent_core::{AgentRunStatus, RunEventCursor, RunScope, StoreError};
use sqlx::Row;
use time::OffsetDateTime;

pub(super) fn encode_scope(scope: &RunScope) -> (&'static str, &str) {
    match scope {
        RunScope::Global => ("global", ""),
        RunScope::User(id) => ("user", id),
        RunScope::Tenant(id) => ("tenant", id),
    }
}

pub(super) fn encode_run_status(status: &AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Running => "running",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Skipped => "skipped",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Cancelled => "cancelled",
        AgentRunStatus::TimedOut => "timed_out",
        AgentRunStatus::Abandoned => "abandoned",
    }
}

pub(super) fn sort_key(value: OffsetDateTime) -> Result<i64, StoreError> {
    value
        .unix_timestamp_nanos()
        .try_into()
        .map_err(|_| StoreError::new("timestamp is outside SQLite sort key range"))
}

pub(super) fn encode_record(value: &impl serde::Serialize) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(map_json_err)
}

pub(super) fn decode_records<T>(rows: Vec<sqlx::sqlite::SqliteRow>) -> Result<Vec<T>, StoreError>
where
    T: serde::de::DeserializeOwned,
{
    rows.into_iter()
        .map(|row| decode_record(row.get::<String, _>("record_json")))
        .collect()
}

pub(super) fn decode_record<T>(record_json: String) -> Result<T, StoreError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(&record_json).map_err(map_json_err)
}

pub(super) fn checked_limit(limit: usize) -> Result<i64, StoreError> {
    limit
        .try_into()
        .map_err(|_| StoreError::new("run list limit exceeds SQLite integer range"))
}

pub(super) fn checked_cursor(cursor: RunEventCursor) -> Result<i64, StoreError> {
    cursor
        .try_into()
        .map_err(|_| StoreError::new("run event cursor exceeds SQLite integer range"))
}

pub(super) fn checked_cursor_index(cursor: usize) -> Result<i64, StoreError> {
    cursor
        .try_into()
        .map_err(|_| StoreError::new("run event cursor exceeds SQLite integer range"))
}

pub(super) fn checked_record_version(version: u64) -> Result<i64, StoreError> {
    version
        .try_into()
        .map_err(|_| StoreError::new("run record version exceeds SQLite integer range"))
}

pub(super) fn decode_cursor(cursor: i64) -> Result<RunEventCursor, StoreError> {
    cursor
        .try_into()
        .map_err(|_| StoreError::new("stored run event cursor is negative"))
}

pub(super) fn lease_duration(ttl: Duration) -> time::Duration {
    time::Duration::seconds(ttl.as_secs().max(1) as i64)
}

pub(super) fn map_sqlx_err(err: sqlx::Error) -> StoreError {
    StoreError::new(err.to_string())
}

pub(super) fn map_io_err(err: std::io::Error) -> StoreError {
    StoreError::new(err.to_string())
}

fn map_json_err(err: serde_json::Error) -> StoreError {
    StoreError::new(err.to_string())
}
