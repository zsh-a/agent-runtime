use std::time::Duration;

use agent_core::{
    AgentLockStore, AgentProposalStore, AgentRunEventStore, AgentRunRecord, AgentRunStore,
    AgentSessionStore, AgentStateStore, ProposalEnvelope, ProposalId, RunEventCursor,
    RunEventRecord, RunId, RunLease, RunScope, SessionId, SessionRecord, StepRecord, StoreError,
    ThreadId, ThreadRecord, TraceEvent,
};
use async_trait::async_trait;
use camino::Utf8Path;
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use time::OffsetDateTime;

struct SqliteMigration {
    version: i64,
    name: &'static str,
    statements: &'static [&'static str],
}

const CURRENT_SCHEMA_STATEMENTS: &[&str] = &[
    r#"
    CREATE TABLE IF NOT EXISTS agent_runs (
        run_id TEXT PRIMARY KEY NOT NULL,
        agent_id TEXT NOT NULL,
        scope_type TEXT NOT NULL,
        scope_id TEXT NOT NULL,
        started_at_sort INTEGER NOT NULL,
        record_json TEXT NOT NULL
    )
    "#,
    r#"
    CREATE INDEX IF NOT EXISTS idx_agent_runs_agent_started
    ON agent_runs(agent_id, started_at_sort DESC)
    "#,
    r#"
    CREATE INDEX IF NOT EXISTS idx_agent_runs_agent_scope_started
    ON agent_runs(agent_id, scope_type, scope_id, started_at_sort DESC)
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS agent_proposals (
        proposal_id TEXT PRIMARY KEY NOT NULL,
        run_id TEXT NOT NULL,
        created_at_sort INTEGER NOT NULL,
        record_json TEXT NOT NULL
    )
    "#,
    r#"
    CREATE INDEX IF NOT EXISTS idx_agent_proposals_run_created
    ON agent_proposals(run_id, created_at_sort ASC)
    "#,
    r#"
    CREATE INDEX IF NOT EXISTS idx_agent_proposals_created
    ON agent_proposals(created_at_sort ASC)
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS agent_sessions (
        session_id TEXT PRIMARY KEY NOT NULL,
        updated_at_sort INTEGER NOT NULL,
        record_json TEXT NOT NULL
    )
    "#,
    r#"
    CREATE INDEX IF NOT EXISTS idx_agent_sessions_updated
    ON agent_sessions(updated_at_sort DESC)
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS agent_threads (
        thread_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL,
        created_at_sort INTEGER NOT NULL,
        record_json TEXT NOT NULL
    )
    "#,
    r#"
    CREATE INDEX IF NOT EXISTS idx_agent_threads_session_created
    ON agent_threads(session_id, created_at_sort ASC)
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS agent_steps (
        step_id TEXT PRIMARY KEY NOT NULL,
        thread_id TEXT NOT NULL,
        created_at_sort INTEGER NOT NULL,
        record_json TEXT NOT NULL
    )
    "#,
    r#"
    CREATE INDEX IF NOT EXISTS idx_agent_steps_thread_created
    ON agent_steps(thread_id, created_at_sort ASC)
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS agent_state (
        agent_id TEXT NOT NULL,
        state_key TEXT NOT NULL,
        value_json TEXT NOT NULL,
        PRIMARY KEY(agent_id, state_key)
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS agent_locks (
        lock_key TEXT PRIMARY KEY NOT NULL,
        owner TEXT NOT NULL,
        acquired_at_sort INTEGER NOT NULL,
        expires_at_sort INTEGER NOT NULL,
        record_json TEXT NOT NULL
    )
    "#,
    r#"
    CREATE INDEX IF NOT EXISTS idx_agent_locks_expires
    ON agent_locks(expires_at_sort ASC)
    "#,
];

const SQLITE_MIGRATIONS: &[SqliteMigration] = &[
    SqliteMigration {
        version: 2,
        name: "current_schema",
        statements: CURRENT_SCHEMA_STATEMENTS,
    },
    SqliteMigration {
        version: 3,
        name: "run_event_logs",
        statements: &[
            r#"
        CREATE TABLE IF NOT EXISTS agent_run_event_logs (
            run_id TEXT PRIMARY KEY NOT NULL
        )
        "#,
            r#"
        CREATE TABLE IF NOT EXISTS agent_run_events (
            run_id TEXT NOT NULL,
            cursor INTEGER NOT NULL,
            event_json TEXT NOT NULL,
            PRIMARY KEY(run_id, cursor)
        )
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_run_cursor
        ON agent_run_events(run_id, cursor ASC)
        "#,
        ],
    },
];

const SCHEMA_VERSION: i64 = SQLITE_MIGRATIONS[SQLITE_MIGRATIONS.len() - 1].version;

#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub async fn open(path: impl AsRef<Utf8Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_str().is_empty()
        {
            fs_err::tokio::create_dir_all(parent)
                .await
                .map_err(map_io_err)?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path.as_std_path())
            .create_if_missing(true);
        Self::connect(options, SqlitePoolOptions::new().max_connections(5)).await
    }

    pub async fn in_memory() -> Result<Self, StoreError> {
        let pool_options = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .idle_timeout(None::<Duration>)
            .max_lifetime(None::<Duration>);
        Self::connect(SqliteConnectOptions::new().in_memory(true), pool_options).await
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn supported_schema_version() -> i64 {
        SCHEMA_VERSION
    }

    pub async fn schema_version(&self) -> Result<i64, StoreError> {
        sqlite_schema_version(&self.pool).await
    }

    async fn connect(
        options: SqliteConnectOptions,
        pool_options: SqlitePoolOptions,
    ) -> Result<Self, StoreError> {
        let pool = pool_options
            .connect_with(options)
            .await
            .map_err(map_sqlx_err)?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<(), StoreError> {
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        let existing_version = self.schema_version().await?;
        if existing_version > SCHEMA_VERSION {
            return Err(StoreError::new(format!(
                "SQLite store schema version {existing_version} is newer than supported version {SCHEMA_VERSION}"
            )));
        }
        validate_sqlite_migrations()?;
        for migration in SQLITE_MIGRATIONS {
            if migration.version > existing_version {
                apply_sqlite_migration(&self.pool, migration).await?;
            }
        }
        Ok(())
    }
}

fn validate_sqlite_migrations() -> Result<(), StoreError> {
    let mut previous_migration_version = 0;
    for migration in SQLITE_MIGRATIONS {
        if migration.version <= previous_migration_version {
            return Err(StoreError::new(
                "SQLite migrations must be ordered by increasing version",
            ));
        }
        previous_migration_version = migration.version;
    }
    Ok(())
}

async fn apply_sqlite_migration(
    pool: &SqlitePool,
    migration: &SqliteMigration,
) -> Result<(), StoreError> {
    let mut transaction = pool.begin().await.map_err(map_sqlx_err)?;
    for statement in migration.statements {
        sqlx::query(statement)
            .execute(&mut *transaction)
            .await
            .map_err(|err| map_migration_err(migration, err))?;
    }
    sqlx::query(&format!("PRAGMA user_version = {}", migration.version))
        .execute(&mut *transaction)
        .await
        .map_err(|err| map_migration_err(migration, err))?;
    transaction.commit().await.map_err(map_sqlx_err)?;
    Ok(())
}

#[async_trait]
impl AgentStateStore for SqliteStore {
    async fn load(
        &self,
        agent_id: &str,
        key: &str,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        let row =
            sqlx::query("SELECT value_json FROM agent_state WHERE agent_id = ? AND state_key = ?")
                .bind(agent_id)
                .bind(key)
                .fetch_optional(&self.pool)
                .await
                .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("value_json")))
            .transpose()
    }

    async fn save(
        &self,
        agent_id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), StoreError> {
        let value_json = encode_record(&value)?;
        sqlx::query(
            r#"
            INSERT INTO agent_state(agent_id, state_key, value_json)
            VALUES (?, ?, ?)
            ON CONFLICT(agent_id, state_key) DO UPDATE SET
                value_json = excluded.value_json
            "#,
        )
        .bind(agent_id)
        .bind(key)
        .bind(value_json)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        Ok(())
    }
}

#[async_trait]
impl AgentRunEventStore for SqliteStore {
    async fn append_run_event(&self, run_id: &RunId, event: TraceEvent) -> Result<(), StoreError> {
        let event_json = encode_record(&event)?;
        let mut transaction = self.pool.begin().await.map_err(map_sqlx_err)?;
        touch_run_event_log(&mut transaction, run_id)
            .await
            .map_err(map_sqlx_err)?;
        sqlx::query(
            r#"
            INSERT INTO agent_run_events(run_id, cursor, event_json)
            SELECT ?, COALESCE(MAX(cursor), 0) + 1, ?
            FROM agent_run_events
            WHERE run_id = ?
            "#,
        )
        .bind(&run_id.0)
        .bind(event_json)
        .bind(&run_id.0)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_err)?;
        transaction.commit().await.map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn replace_run_events(
        &self,
        run_id: &RunId,
        events: Vec<TraceEvent>,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await.map_err(map_sqlx_err)?;
        touch_run_event_log(&mut transaction, run_id)
            .await
            .map_err(map_sqlx_err)?;
        sqlx::query("DELETE FROM agent_run_events WHERE run_id = ?")
            .bind(&run_id.0)
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_err)?;
        for (index, event) in events.into_iter().enumerate() {
            let cursor = checked_cursor_index(index.saturating_add(1))?;
            let event_json = encode_record(&event)?;
            sqlx::query(
                r#"
                INSERT INTO agent_run_events(run_id, cursor, event_json)
                VALUES (?, ?, ?)
                "#,
            )
            .bind(&run_id.0)
            .bind(cursor)
            .bind(event_json)
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_err)?;
        }
        transaction.commit().await.map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn list_run_events_after(
        &self,
        run_id: &RunId,
        after: RunEventCursor,
    ) -> Result<Option<Vec<RunEventRecord>>, StoreError> {
        let exists = sqlx::query("SELECT 1 FROM agent_run_event_logs WHERE run_id = ?")
            .bind(&run_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?
            .is_some();
        if !exists {
            return Ok(None);
        }
        let rows = sqlx::query(
            r#"
            SELECT cursor, event_json
            FROM agent_run_events
            WHERE run_id = ? AND cursor > ?
            ORDER BY cursor ASC
            "#,
        )
        .bind(&run_id.0)
        .bind(checked_cursor(after)?)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        rows.into_iter()
            .map(|row| {
                Ok(RunEventRecord {
                    cursor: decode_cursor(row.get::<i64, _>("cursor"))?,
                    event: decode_record(row.get::<String, _>("event_json"))?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()
            .map(Some)
    }
}

#[async_trait]
impl AgentLockStore for SqliteStore {
    async fn acquire(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<RunLease>, StoreError> {
        let now = OffsetDateTime::now_utc();
        let lease = RunLease {
            key: key.to_owned(),
            owner: owner.to_owned(),
            acquired_at: now,
            expires_at: now + lease_duration(ttl),
        };
        let record_json = encode_record(&lease)?;
        let row = sqlx::query(
            r#"
            INSERT INTO agent_locks(
                lock_key,
                owner,
                acquired_at_sort,
                expires_at_sort,
                record_json
            )
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(lock_key) DO UPDATE SET
                owner = excluded.owner,
                acquired_at_sort = excluded.acquired_at_sort,
                expires_at_sort = excluded.expires_at_sort,
                record_json = excluded.record_json
            WHERE agent_locks.owner = excluded.owner
                OR agent_locks.expires_at_sort <= ?
            RETURNING record_json
            "#,
        )
        .bind(key)
        .bind(owner)
        .bind(sort_key(lease.acquired_at)?)
        .bind(sort_key(lease.expires_at)?)
        .bind(record_json)
        .bind(sort_key(now)?)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }

    async fn renew(&self, lease: &RunLease, ttl: Duration) -> Result<(), StoreError> {
        let mut renewed = lease.clone();
        renewed.expires_at = OffsetDateTime::now_utc() + lease_duration(ttl);
        let record_json = encode_record(&renewed)?;
        sqlx::query(
            r#"
            UPDATE agent_locks
            SET expires_at_sort = ?, record_json = ?
            WHERE lock_key = ? AND owner = ?
            "#,
        )
        .bind(sort_key(renewed.expires_at)?)
        .bind(record_json)
        .bind(&lease.key)
        .bind(&lease.owner)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn release(&self, lease: RunLease) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM agent_locks WHERE lock_key = ? AND owner = ?")
            .bind(&lease.key)
            .bind(&lease.owner)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        Ok(())
    }
}

#[async_trait]
impl AgentRunStore for SqliteStore {
    async fn create_run(&self, run: AgentRunRecord) -> Result<(), StoreError> {
        upsert_run(&self.pool, run).await
    }

    async fn update_run(&self, run: AgentRunRecord) -> Result<(), StoreError> {
        upsert_run(&self.pool, run).await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<AgentRunRecord>, StoreError> {
        let row = sqlx::query("SELECT record_json FROM agent_runs WHERE run_id = ?")
            .bind(&run_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }

    async fn list_runs(
        &self,
        agent_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>, StoreError> {
        let limit = limit.unwrap_or(i64::MAX as usize);
        let rows = match agent_id {
            Some(agent_id) => {
                sqlx::query(
                    r#"
                    SELECT record_json FROM agent_runs
                    WHERE agent_id = ?
                    ORDER BY started_at_sort DESC
                    LIMIT ?
                    "#,
                )
                .bind(agent_id)
                .bind(checked_limit(limit)?)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query(
                    r#"
                    SELECT record_json FROM agent_runs
                    ORDER BY started_at_sort DESC
                    LIMIT ?
                    "#,
                )
                .bind(checked_limit(limit)?)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }

    async fn last_run(
        &self,
        agent_id: &str,
        scope: &RunScope,
    ) -> Result<Option<AgentRunRecord>, StoreError> {
        let (scope_type, scope_id) = encode_scope(scope);
        let row = sqlx::query(
            r#"
            SELECT record_json FROM agent_runs
            WHERE agent_id = ? AND scope_type = ? AND scope_id = ?
            ORDER BY started_at_sort DESC
            LIMIT 1
            "#,
        )
        .bind(agent_id)
        .bind(scope_type)
        .bind(scope_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }
}

#[async_trait]
impl AgentProposalStore for SqliteStore {
    async fn create_proposal(&self, proposal: ProposalEnvelope) -> Result<(), StoreError> {
        upsert_proposal(&self.pool, proposal).await
    }

    async fn update_proposal(&self, proposal: ProposalEnvelope) -> Result<(), StoreError> {
        upsert_proposal(&self.pool, proposal).await
    }

    async fn get_proposal(
        &self,
        proposal_id: &ProposalId,
    ) -> Result<Option<ProposalEnvelope>, StoreError> {
        let row = sqlx::query("SELECT record_json FROM agent_proposals WHERE proposal_id = ?")
            .bind(&proposal_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }

    async fn list_proposals(
        &self,
        run_id: Option<&RunId>,
    ) -> Result<Vec<ProposalEnvelope>, StoreError> {
        let rows = match run_id {
            Some(run_id) => {
                sqlx::query(
                    r#"
                    SELECT record_json FROM agent_proposals
                    WHERE run_id = ?
                    ORDER BY created_at_sort ASC
                    "#,
                )
                .bind(&run_id.0)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query(
                    r#"
                    SELECT record_json FROM agent_proposals
                    ORDER BY created_at_sort ASC
                    "#,
                )
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }
}

#[async_trait]
impl AgentSessionStore for SqliteStore {
    async fn create_session(&self, session: SessionRecord) -> Result<(), StoreError> {
        let record_json = encode_record(&session)?;
        sqlx::query(
            r#"
            INSERT INTO agent_sessions(session_id, updated_at_sort, record_json)
            VALUES (?, ?, ?)
            ON CONFLICT(session_id) DO UPDATE SET
                updated_at_sort = excluded.updated_at_sort,
                record_json = excluded.record_json
            "#,
        )
        .bind(&session.session_id.0)
        .bind(sort_key(session.updated_at)?)
        .bind(record_json)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT record_json FROM agent_sessions
            ORDER BY updated_at_sort DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }

    async fn get_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionRecord>, StoreError> {
        let row = sqlx::query("SELECT record_json FROM agent_sessions WHERE session_id = ?")
            .bind(&session_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }

    async fn create_thread(&self, thread: ThreadRecord) -> Result<(), StoreError> {
        let record_json = encode_record(&thread)?;
        sqlx::query(
            r#"
            INSERT INTO agent_threads(thread_id, session_id, created_at_sort, record_json)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(thread_id) DO UPDATE SET
                session_id = excluded.session_id,
                created_at_sort = excluded.created_at_sort,
                record_json = excluded.record_json
            "#,
        )
        .bind(&thread.thread_id.0)
        .bind(&thread.session_id.0)
        .bind(sort_key(thread.created_at)?)
        .bind(record_json)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn list_threads(&self, session_id: &SessionId) -> Result<Vec<ThreadRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT record_json FROM agent_threads
            WHERE session_id = ?
            ORDER BY created_at_sort ASC
            "#,
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }

    async fn get_thread(&self, thread_id: &ThreadId) -> Result<Option<ThreadRecord>, StoreError> {
        let row = sqlx::query("SELECT record_json FROM agent_threads WHERE thread_id = ?")
            .bind(&thread_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_err)?;
        row.map(|row| decode_record(row.get::<String, _>("record_json")))
            .transpose()
    }

    async fn create_step(&self, step: StepRecord) -> Result<(), StoreError> {
        let record_json = encode_record(&step)?;
        sqlx::query(
            r#"
            INSERT INTO agent_steps(step_id, thread_id, created_at_sort, record_json)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(step_id) DO UPDATE SET
                thread_id = excluded.thread_id,
                created_at_sort = excluded.created_at_sort,
                record_json = excluded.record_json
            "#,
        )
        .bind(&step.step_id.0)
        .bind(&step.thread_id.0)
        .bind(sort_key(step.created_at)?)
        .bind(record_json)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        Ok(())
    }

    async fn list_steps(&self, thread_id: &ThreadId) -> Result<Vec<StepRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT record_json FROM agent_steps
            WHERE thread_id = ?
            ORDER BY created_at_sort ASC
            "#,
        )
        .bind(&thread_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_err)?;
        decode_records(rows)
    }
}

async fn upsert_run(pool: &SqlitePool, run: AgentRunRecord) -> Result<(), StoreError> {
    let (scope_type, scope_id) = encode_scope(&run.scope);
    let record_json = encode_record(&run)?;
    sqlx::query(
        r#"
        INSERT INTO agent_runs(run_id, agent_id, scope_type, scope_id, started_at_sort, record_json)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(run_id) DO UPDATE SET
            agent_id = excluded.agent_id,
            scope_type = excluded.scope_type,
            scope_id = excluded.scope_id,
            started_at_sort = excluded.started_at_sort,
            record_json = excluded.record_json
        "#,
    )
    .bind(&run.run_id.0)
    .bind(&run.agent_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(sort_key(run.started_at)?)
    .bind(record_json)
    .execute(pool)
    .await
    .map_err(map_sqlx_err)?;
    Ok(())
}

async fn upsert_proposal(pool: &SqlitePool, proposal: ProposalEnvelope) -> Result<(), StoreError> {
    let record_json = encode_record(&proposal)?;
    sqlx::query(
        r#"
        INSERT INTO agent_proposals(proposal_id, run_id, created_at_sort, record_json)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(proposal_id) DO UPDATE SET
            run_id = excluded.run_id,
            created_at_sort = excluded.created_at_sort,
            record_json = excluded.record_json
        "#,
    )
    .bind(&proposal.proposal_id.0)
    .bind(&proposal.run_id.0)
    .bind(sort_key(proposal.created_at)?)
    .bind(record_json)
    .execute(pool)
    .await
    .map_err(map_sqlx_err)?;
    Ok(())
}

async fn touch_run_event_log(
    transaction: &mut Transaction<'_, Sqlite>,
    run_id: &RunId,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO agent_run_event_logs(run_id) VALUES (?)")
        .bind(&run_id.0)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

fn encode_scope(scope: &RunScope) -> (&'static str, &str) {
    match scope {
        RunScope::Global => ("global", ""),
        RunScope::User(id) => ("user", id),
        RunScope::Tenant(id) => ("tenant", id),
    }
}

fn sort_key(value: OffsetDateTime) -> Result<i64, StoreError> {
    value
        .unix_timestamp_nanos()
        .try_into()
        .map_err(|_| StoreError::new("timestamp is outside SQLite sort key range"))
}

async fn sqlite_schema_version(pool: &SqlitePool) -> Result<i64, StoreError> {
    sqlx::query("PRAGMA user_version")
        .fetch_one(pool)
        .await
        .map(|row| row.get::<i64, _>(0))
        .map_err(map_sqlx_err)
}

fn encode_record(value: &impl serde::Serialize) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(map_json_err)
}

fn decode_records<T>(rows: Vec<sqlx::sqlite::SqliteRow>) -> Result<Vec<T>, StoreError>
where
    T: serde::de::DeserializeOwned,
{
    rows.into_iter()
        .map(|row| decode_record(row.get::<String, _>("record_json")))
        .collect()
}

fn decode_record<T>(record_json: String) -> Result<T, StoreError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(&record_json).map_err(map_json_err)
}

fn checked_limit(limit: usize) -> Result<i64, StoreError> {
    limit
        .try_into()
        .map_err(|_| StoreError::new("run list limit exceeds SQLite integer range"))
}

fn checked_cursor(cursor: RunEventCursor) -> Result<i64, StoreError> {
    cursor
        .try_into()
        .map_err(|_| StoreError::new("run event cursor exceeds SQLite integer range"))
}

fn checked_cursor_index(cursor: usize) -> Result<i64, StoreError> {
    cursor
        .try_into()
        .map_err(|_| StoreError::new("run event cursor exceeds SQLite integer range"))
}

fn decode_cursor(cursor: i64) -> Result<RunEventCursor, StoreError> {
    cursor
        .try_into()
        .map_err(|_| StoreError::new("stored run event cursor is negative"))
}

fn lease_duration(ttl: Duration) -> time::Duration {
    time::Duration::seconds(ttl.as_secs().max(1) as i64)
}

fn map_sqlx_err(err: sqlx::Error) -> StoreError {
    StoreError::new(err.to_string())
}

fn map_migration_err(migration: &SqliteMigration, err: sqlx::Error) -> StoreError {
    StoreError::new(format!(
        "failed to apply SQLite migration {} ({}): {err}",
        migration.version, migration.name
    ))
}

fn map_io_err(err: std::io::Error) -> StoreError {
    StoreError::new(err.to_string())
}

fn map_json_err(err: serde_json::Error) -> StoreError {
    StoreError::new(err.to_string())
}
