use std::time::Duration;

use agent_core::{
    AgentLockStore, AgentProposalStore, AgentRunEventStore, AgentRunRecord, AgentRunStatus,
    AgentRunStore, AgentSessionStore, AgentStateStore, AgentTrace, AgentTraceStore,
    ProposalEnvelope, ProposalId, RunEventCursor, RunEventRecord, RunId, RunLease, RunScope,
    SessionId, SessionRecord, StepRecord, StoreError, ThreadId, ThreadRecord, TraceEvent,
};
use async_trait::async_trait;
use camino::Utf8Path;
use sqlx::{
    Row, Sqlite, SqliteConnection, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use time::OffsetDateTime;

mod codec;
mod events;
mod lock;
mod proposal;
mod run;
mod session;
mod state;
mod trace;

use codec::*;

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
    SqliteMigration {
        version: 4,
        name: "run_status_index",
        statements: &[
            r#"
        ALTER TABLE agent_runs
        ADD COLUMN status TEXT NOT NULL DEFAULT ''
        "#,
            r#"
        UPDATE agent_runs
        SET status = COALESCE(json_extract(record_json, '$.status'), '')
        WHERE status = ''
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS idx_agent_runs_status_started
        ON agent_runs(status, started_at_sort DESC)
            "#,
        ],
    },
    SqliteMigration {
        version: 5,
        name: "agent_traces",
        statements: &[r#"
        CREATE TABLE IF NOT EXISTS agent_traces (
            run_id TEXT PRIMARY KEY NOT NULL,
            record_json TEXT NOT NULL
        )
        "#],
    },
    SqliteMigration {
        version: 6,
        name: "scoped_state",
        statements: &[r#"
            CREATE TABLE IF NOT EXISTS agent_state_scoped (
                agent_id TEXT NOT NULL,
                scope_type TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                state_key TEXT NOT NULL,
                value_json TEXT NOT NULL,
                PRIMARY KEY(agent_id, scope_type, scope_id, state_key)
            )
            "#],
    },
    SqliteMigration {
        version: 7,
        name: "run_idempotency",
        statements: &[r#"
            CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_runs_idempotency
            ON agent_runs(agent_id, scope_type, scope_id, json_extract(record_json, '$.idempotency_key'))
            WHERE json_extract(record_json, '$.idempotency_key') IS NOT NULL
        "#],
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
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        Self::connect(options, SqlitePoolOptions::new().max_connections(5)).await
    }

    pub async fn in_memory() -> Result<Self, StoreError> {
        let pool_options = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .idle_timeout(None::<Duration>)
            .max_lifetime(None::<Duration>);
        Self::connect(
            SqliteConnectOptions::new()
                .in_memory(true)
                .foreign_keys(true)
                .busy_timeout(Duration::from_secs(5)),
            pool_options,
        )
        .await
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
        validate_sqlite_migrations()?;
        let mut connection = self.pool.acquire().await.map_err(map_sqlx_err)?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .map_err(map_sqlx_err)?;

        let migration_result = async {
            let existing_version = sqlite_connection_schema_version(&mut connection).await?;
            if existing_version > SCHEMA_VERSION {
                return Err(StoreError::new(format!(
                    "SQLite store schema version {existing_version} is newer than supported version {SCHEMA_VERSION}"
                )));
            }
            for migration in SQLITE_MIGRATIONS {
                if migration.version > existing_version {
                    apply_sqlite_migration(&mut connection, migration).await?;
                }
            }
            Ok(())
        }
        .await;

        match migration_result {
            Ok(()) => {
                sqlx::query("COMMIT")
                    .execute(&mut *connection)
                    .await
                    .map_err(map_sqlx_err)?;
                Ok(())
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                Err(error)
            }
        }
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
    connection: &mut SqliteConnection,
    migration: &SqliteMigration,
) -> Result<(), StoreError> {
    for statement in migration.statements {
        sqlx::query(statement)
            .execute(&mut *connection)
            .await
            .map_err(|err| map_migration_err(migration, err))?;
    }
    sqlx::query(&format!("PRAGMA user_version = {}", migration.version))
        .execute(&mut *connection)
        .await
        .map_err(|err| map_migration_err(migration, err))?;
    Ok(())
}

async fn insert_proposal(pool: &SqlitePool, proposal: ProposalEnvelope) -> Result<(), StoreError> {
    let record_json = encode_record(&proposal)?;
    sqlx::query(
        r#"
        INSERT INTO agent_proposals(proposal_id, run_id, created_at_sort, record_json)
        VALUES (?, ?, ?, ?)
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

async fn update_proposal_cas(
    pool: &SqlitePool,
    proposal: ProposalEnvelope,
    expected_version: u64,
) -> Result<bool, StoreError> {
    if proposal.version != expected_version + 1 {
        return Ok(false);
    }
    let record_json = encode_record(&proposal)?;
    let result = sqlx::query(
        r#"
        UPDATE agent_proposals
        SET run_id = ?, created_at_sort = ?, record_json = ?
        WHERE proposal_id = ?
          AND CAST(json_extract(record_json, '$.version') AS INTEGER) = ?
        "#,
    )
    .bind(&proposal.run_id.0)
    .bind(sort_key(proposal.created_at)?)
    .bind(record_json)
    .bind(&proposal.proposal_id.0)
    .bind(
        i64::try_from(expected_version)
            .map_err(|_| StoreError::new("proposal version overflow"))?,
    )
    .execute(pool)
    .await
    .map_err(map_sqlx_err)?;
    Ok(result.rows_affected() == 1)
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

async fn sqlite_schema_version(pool: &SqlitePool) -> Result<i64, StoreError> {
    sqlx::query("PRAGMA user_version")
        .fetch_one(pool)
        .await
        .map(|row| row.get::<i64, _>(0))
        .map_err(map_sqlx_err)
}

async fn sqlite_connection_schema_version(
    connection: &mut SqliteConnection,
) -> Result<i64, StoreError> {
    sqlx::query("PRAGMA user_version")
        .fetch_one(connection)
        .await
        .map(|row| row.get::<i64, _>(0))
        .map_err(map_sqlx_err)
}

fn map_migration_err(migration: &SqliteMigration, err: sqlx::Error) -> StoreError {
    StoreError::new(format!(
        "failed to apply SQLite migration {} ({}): {err}",
        migration.version, migration.name
    ))
}
