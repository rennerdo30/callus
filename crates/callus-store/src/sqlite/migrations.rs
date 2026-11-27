//! Versioned, idempotent SQLite schema migrations.

use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;

/// Default embedding dimension matching `bge-small-en-v1.5` (384).
pub const DEFAULT_EMBEDDING_DIMS: usize = 384;

const SCHEMA_VERSION: u32 = 1;

const BASE_SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS error_patterns (
    id               TEXT PRIMARY KEY,
    signature_hash   TEXT NOT NULL UNIQUE,
    title            TEXT NOT NULL,
    error_template   TEXT NOT NULL,
    tool_name        TEXT NOT NULL,
    scope            TEXT NOT NULL,
    repo_hash        TEXT,
    occurrence_count INTEGER NOT NULL DEFAULT 1,
    first_seen_at    TEXT NOT NULL,
    last_seen_at     TEXT NOT NULL,
    embedding_status TEXT NOT NULL DEFAULT 'pending',
    updated_at       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_error_patterns_scope_repo
    ON error_patterns (scope, repo_hash);
CREATE INDEX IF NOT EXISTS idx_error_patterns_embedding_status
    ON error_patterns (embedding_status, updated_at);
CREATE INDEX IF NOT EXISTS idx_error_patterns_last_seen
    ON error_patterns (last_seen_at DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS error_patterns_fts USING fts5(
    pattern_id UNINDEXED,
    title,
    error_template,
    tool_name,
    tokenize = 'porter unicode61'
);

CREATE TABLE IF NOT EXISTS fixes (
    id             TEXT PRIMARY KEY,
    summary        TEXT NOT NULL,
    fix_type       TEXT NOT NULL,
    pattern_before TEXT,
    pattern_after  TEXT,
    guard_regex    TEXT,
    status         TEXT NOT NULL,
    scope          TEXT NOT NULL,
    source         TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_fixes_type_status
    ON fixes (fix_type, status);

CREATE TABLE IF NOT EXISTS graph_nodes (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    label      TEXT NOT NULL,
    props_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_graph_nodes_kind
    ON graph_nodes (kind);

CREATE TABLE IF NOT EXISTS graph_edges (
    src_id       TEXT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    edge_type    TEXT NOT NULL,
    dst_id       TEXT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    worked_count INTEGER NOT NULL DEFAULT 0,
    failed_count INTEGER NOT NULL DEFAULT 0,
    hit_count    INTEGER NOT NULL DEFAULT 0,
    last_seen_at TEXT NOT NULL,
    PRIMARY KEY (src_id, edge_type, dst_id)
);

CREATE INDEX IF NOT EXISTS idx_graph_edges_src_type
    ON graph_edges (src_id, edge_type);
CREATE INDEX IF NOT EXISTS idx_graph_edges_dst_type
    ON graph_edges (dst_id, edge_type);

CREATE TABLE IF NOT EXISTS episodes (
    id               TEXT PRIMARY KEY,
    session_id       TEXT NOT NULL,
    cli              TEXT NOT NULL,
    model            TEXT,
    os               TEXT NOT NULL,
    shell            TEXT NOT NULL,
    tool_name        TEXT NOT NULL,
    phase            TEXT NOT NULL,
    pattern_id       TEXT,
    injected_fix_id  TEXT,
    command_scrubbed TEXT,
    created_at       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_episodes_session_created
    ON episodes (session_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_episodes_created
    ON episodes (created_at);

CREATE TABLE IF NOT EXISTS sync_outbox (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type  TEXT NOT NULL,
    entity_id    TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at   TEXT NOT NULL
);
";

/// Configures SQLite pragmas, validates `embedding_dims`, and applies migrations.
pub(crate) fn apply_migrations(
    conn: &Connection,
    embedding_dims: usize,
    enable_wal: bool,
) -> Result<(), StoreError> {
    if embedding_dims == 0 {
        return Err(StoreError::InvalidData {
            context: "embedding_dims",
            detail: "embedding_dims must be greater than 0".to_owned(),
        });
    }

    if enable_wal {
        conn.pragma_update(None, "journal_mode", "WAL")?;
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000_i64)?;

    conn.execute_batch(BASE_SCHEMA_SQL)?;

    // Enforce immutable embedding dimensions across restarts so vectors are never corrupted.
    let stored_dims: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'embedding_dims'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(raw_dims) = stored_dims {
        let expected: usize = raw_dims.parse().map_err(|err| StoreError::InvalidData {
            context: "schema_meta.embedding_dims",
            detail: format!("{raw_dims:?}: {err}"),
        })?;
        if expected != embedding_dims {
            return Err(StoreError::DimensionMismatch {
                expected,
                actual: embedding_dims,
            });
        }
    } else {
        conn.execute(
            "INSERT INTO schema_meta (key, value) VALUES ('embedding_dims', ?1)",
            params![embedding_dims.to_string()],
        )?;
    }

    let vec_sql = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS error_patterns_vec USING vec0(\
         pattern_id TEXT PRIMARY KEY, \
         embedding float[{embedding_dims}] distance_metric=cosine\
         );"
    );
    conn.execute_batch(&vec_sql)?;

    conn.execute(
        "INSERT INTO schema_meta (key, value) VALUES ('schema_version', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SCHEMA_VERSION.to_string()],
    )?;

    Ok(())
}
