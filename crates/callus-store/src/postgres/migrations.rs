//! Versioned, idempotent PostgreSQL migrations (`pgvector`, `tsvector`, `pg_trgm`, graph).

use sqlx::{AssertSqlSafe, PgPool};

use crate::StoreError;

const SCHEMA_VERSION: u32 = 1;

const STATIC_STATEMENTS: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS idx_pg_error_patterns_scope_repo \
     ON error_patterns (scope, repo_hash)",
    "CREATE INDEX IF NOT EXISTS idx_pg_error_patterns_updated \
     ON error_patterns (updated_at)",
    "CREATE INDEX IF NOT EXISTS idx_pg_error_patterns_tsv \
     ON error_patterns USING GIN (search_tsv)",
    "CREATE INDEX IF NOT EXISTS idx_pg_error_patterns_trgm \
     ON error_patterns USING GIN (error_template gin_trgm_ops)",
    "CREATE INDEX IF NOT EXISTS idx_pg_error_patterns_vec \
     ON error_patterns USING hnsw (embedding vector_cosine_ops)",
    "CREATE TABLE IF NOT EXISTS fixes ( \
        id             TEXT PRIMARY KEY, \
        summary        TEXT NOT NULL, \
        fix_type       TEXT NOT NULL, \
        pattern_before TEXT, \
        pattern_after  TEXT, \
        guard_regex    TEXT, \
        status         TEXT NOT NULL, \
        scope          TEXT NOT NULL, \
        source         TEXT NOT NULL, \
        created_at     TEXT NOT NULL, \
        updated_at     TEXT NOT NULL \
    )",
    "CREATE INDEX IF NOT EXISTS idx_pg_fixes_type_status \
     ON fixes (fix_type, status)",
    "CREATE INDEX IF NOT EXISTS idx_pg_fixes_updated \
     ON fixes (updated_at)",
    "CREATE TABLE IF NOT EXISTS graph_nodes ( \
        id         TEXT PRIMARY KEY, \
        kind       TEXT NOT NULL, \
        label      TEXT NOT NULL, \
        props_json JSONB NOT NULL DEFAULT '{}'::jsonb \
    )",
    "CREATE INDEX IF NOT EXISTS idx_pg_graph_nodes_kind \
     ON graph_nodes (kind)",
    "CREATE TABLE IF NOT EXISTS graph_edges ( \
        src_id       TEXT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE, \
        edge_type    TEXT NOT NULL, \
        dst_id       TEXT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE, \
        worked_count BIGINT NOT NULL DEFAULT 0, \
        failed_count BIGINT NOT NULL DEFAULT 0, \
        hit_count    BIGINT NOT NULL DEFAULT 0, \
        last_seen_at TEXT NOT NULL, \
        PRIMARY KEY (src_id, edge_type, dst_id) \
    )",
    "CREATE INDEX IF NOT EXISTS idx_pg_graph_edges_src_type \
     ON graph_edges (src_id, edge_type)",
    "CREATE INDEX IF NOT EXISTS idx_pg_graph_edges_dst_type \
     ON graph_edges (dst_id, edge_type)",
    "CREATE TABLE IF NOT EXISTS episodes ( \
        id               TEXT PRIMARY KEY, \
        session_id       TEXT NOT NULL, \
        cli              TEXT NOT NULL, \
        model            TEXT, \
        os               TEXT NOT NULL, \
        shell            TEXT NOT NULL, \
        tool_name        TEXT NOT NULL, \
        phase            TEXT NOT NULL, \
        pattern_id       TEXT, \
        injected_fix_id  TEXT, \
        command_scrubbed TEXT, \
        created_at       TEXT NOT NULL \
    )",
    "CREATE INDEX IF NOT EXISTS idx_pg_episodes_session_created \
     ON episodes (session_id, created_at DESC)",
    "CREATE TABLE IF NOT EXISTS sync_outbox ( \
        id           BIGSERIAL PRIMARY KEY, \
        entity_type  TEXT NOT NULL, \
        entity_id    TEXT NOT NULL, \
        payload_json TEXT NOT NULL, \
        created_at   TEXT NOT NULL \
    )",
];

/// Applies extensions, validates `embedding_dims`, and ensures all tables & indexes exist.
pub(crate) async fn apply_migrations(
    pool: &PgPool,
    embedding_dims: usize,
) -> Result<(), StoreError> {
    if embedding_dims == 0 {
        return Err(StoreError::InvalidData {
            context: "embedding_dims",
            detail: "embedding_dims must be greater than 0".to_owned(),
        });
    }

    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
        .execute(pool)
        .await?;
    sqlx::query("CREATE EXTENSION IF NOT EXISTS pg_trgm")
        .execute(pool)
        .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS schema_meta ( \
            key   TEXT PRIMARY KEY, \
            value TEXT NOT NULL \
        )",
    )
    .execute(pool)
    .await?;

    let stored_dims: Option<String> =
        sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'embedding_dims'")
            .fetch_optional(pool)
            .await?;

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
        sqlx::query("INSERT INTO schema_meta (key, value) VALUES ('embedding_dims', $1)")
            .bind(embedding_dims.to_string())
            .execute(pool)
            .await?;
    }

    // `embedding_dims` is a validated positive integer (`usize`), never user text.
    let create_patterns_sql = format!(
        "CREATE TABLE IF NOT EXISTS error_patterns ( \
            id               TEXT PRIMARY KEY, \
            signature_hash   TEXT NOT NULL UNIQUE, \
            title            TEXT NOT NULL, \
            error_template   TEXT NOT NULL, \
            tool_name        TEXT NOT NULL, \
            scope            TEXT NOT NULL, \
            repo_hash        TEXT, \
            occurrence_count BIGINT NOT NULL DEFAULT 1, \
            first_seen_at    TEXT NOT NULL, \
            last_seen_at     TEXT NOT NULL, \
            embedding_status TEXT NOT NULL DEFAULT 'pending', \
            updated_at       TEXT NOT NULL, \
            embedding        vector({embedding_dims}), \
            search_tsv       tsvector GENERATED ALWAYS AS ( \
                to_tsvector('english', coalesce(title, '') || ' ' || coalesce(error_template, '') || ' ' || coalesce(tool_name, '')) \
            ) STORED \
        )"
    );
    sqlx::query(AssertSqlSafe(create_patterns_sql))
        .execute(pool)
        .await?;

    for &sql in STATIC_STATEMENTS {
        sqlx::query(sql).execute(pool).await?;
    }

    sqlx::query(
        "INSERT INTO schema_meta (key, value) VALUES ('schema_version', $1) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(SCHEMA_VERSION.to_string())
    .execute(pool)
    .await?;

    Ok(())
}
