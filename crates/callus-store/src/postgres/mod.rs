//! PostgreSQL (`pgvector` + `tsvector` + `pg_trgm` + graph) implementation of
//! [`KnowledgeStore`] for the central Docker team server.

pub mod migrations;

use std::{
    collections::{HashSet, VecDeque},
    fmt::Write as _,
    str::FromStr,
    time::Duration,
};

use async_trait::async_trait;
use callus_core::{
    EdgeType, EmbeddingStatus, Episode, ErrorPattern, Fix, FixStatus, GraphEdge, GraphNode, Scope,
};
use sqlx::{
    PgPool, Row as _,
    postgres::{PgPoolOptions, PgRow},
};

use migrations::apply_migrations;

use crate::{
    Bm25Hit, EdgeDirection, FixCandidate, KnowledgeStore, Neighbor, OutboxEntry, StoreError,
    VectorHit,
};

/// Central PostgreSQL store backed by `sqlx` + `pgvector` + `tsvector`/`pg_trgm`.
#[derive(Debug, Clone)]
pub struct PgStore {
    pool: PgPool,
    embedding_dims: usize,
}

impl PgStore {
    /// Connects to PostgreSQL with LiteVM-lean pool defaults and runs migrations.
    pub async fn connect(
        database_url: &str,
        embedding_dims: usize,
        max_connections: u32,
    ) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.max(2))
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url)
            .await?;
        Self::from_pool(pool, embedding_dims).await
    }

    /// Wraps an existing [`PgPool`] and ensures idempotent migrations are applied.
    pub async fn from_pool(pool: PgPool, embedding_dims: usize) -> Result<Self, StoreError> {
        apply_migrations(&pool, embedding_dims).await?;
        Ok(Self {
            pool,
            embedding_dims,
        })
    }

    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Lightweight connectivity check for `/healthz`.
    pub async fn ping(&self) -> Result<(), StoreError> {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(())
    }
}

/// Encodes an `f32` embedding slice as a `pgvector` text literal (`[0.1,0.2,...]`).
pub fn encode_pgvector(values: &[f32]) -> String {
    let mut out = String::with_capacity(values.len() * 8 + 2);
    out.push('[');
    for (idx, val) in values.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let _ = write!(out, "{val}");
    }
    out.push(']');
    out
}

fn u64_to_i64(value: u64, context: &'static str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|err| StoreError::InvalidData {
        context,
        detail: err.to_string(),
    })
}

fn usize_to_i64(value: usize, context: &'static str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|err| StoreError::InvalidData {
        context,
        detail: err.to_string(),
    })
}

fn i64_to_u64(value: i64, context: &'static str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|err| StoreError::InvalidData {
        context,
        detail: err.to_string(),
    })
}

fn parse_enum<T: FromStr<Err = callus_core::CallusError>>(
    raw: &str,
    context: &'static str,
) -> Result<T, StoreError> {
    T::from_str(raw).map_err(|err| StoreError::InvalidData {
        context,
        detail: err.to_string(),
    })
}

fn row_to_pattern(row: &PgRow) -> Result<ErrorPattern, StoreError> {
    let scope_raw: String = row.try_get("scope")?;
    let count_raw: i64 = row.try_get("occurrence_count")?;
    let status_raw: String = row.try_get("embedding_status")?;
    Ok(ErrorPattern {
        id: row.try_get("id")?,
        signature_hash: row.try_get("signature_hash")?,
        title: row.try_get("title")?,
        error_template: row.try_get("error_template")?,
        tool_name: row.try_get("tool_name")?,
        scope: parse_enum(&scope_raw, "error_patterns.scope")?,
        repo_hash: row.try_get("repo_hash")?,
        occurrence_count: i64_to_u64(count_raw, "error_patterns.occurrence_count")?,
        first_seen_at: row.try_get("first_seen_at")?,
        last_seen_at: row.try_get("last_seen_at")?,
        embedding_status: parse_enum(&status_raw, "error_patterns.embedding_status")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_fix(row: &PgRow) -> Result<Fix, StoreError> {
    let fix_type_raw: String = row.try_get("fix_type")?;
    let status_raw: String = row.try_get("status")?;
    let scope_raw: String = row.try_get("scope")?;
    let source_raw: String = row.try_get("source")?;
    Ok(Fix {
        id: row.try_get("id")?,
        summary: row.try_get("summary")?,
        fix_type: parse_enum(&fix_type_raw, "fixes.fix_type")?,
        pattern_before: row.try_get("pattern_before")?,
        pattern_after: row.try_get("pattern_after")?,
        guard_regex: row.try_get("guard_regex")?,
        status: parse_enum(&status_raw, "fixes.status")?,
        scope: parse_enum(&scope_raw, "fixes.scope")?,
        source: parse_enum(&source_raw, "fixes.source")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_node(row: &PgRow) -> Result<GraphNode, StoreError> {
    let kind_raw: String = row.try_get("kind")?;
    let props: serde_json::Value = row.try_get("props_json")?;
    Ok(GraphNode {
        id: row.try_get("id")?,
        kind: parse_enum(&kind_raw, "graph_nodes.kind")?,
        label: row.try_get("label")?,
        props_json: props,
    })
}

fn row_to_edge(row: &PgRow) -> Result<GraphEdge, StoreError> {
    let edge_type_raw: String = row.try_get("edge_type")?;
    let worked: i64 = row.try_get("worked_count")?;
    let failed: i64 = row.try_get("failed_count")?;
    let hits: i64 = row.try_get("hit_count")?;
    Ok(GraphEdge {
        src_id: row.try_get("src_id")?,
        edge_type: parse_enum(&edge_type_raw, "graph_edges.edge_type")?,
        dst_id: row.try_get("dst_id")?,
        worked_count: i64_to_u64(worked, "graph_edges.worked_count")?,
        failed_count: i64_to_u64(failed, "graph_edges.failed_count")?,
        hit_count: i64_to_u64(hits, "graph_edges.hit_count")?,
        last_seen_at: row.try_get("last_seen_at")?,
    })
}

fn row_to_episode(row: &PgRow) -> Result<Episode, StoreError> {
    let cli_raw: String = row.try_get("cli")?;
    let os_raw: String = row.try_get("os")?;
    let shell_raw: String = row.try_get("shell")?;
    let phase_raw: String = row.try_get("phase")?;
    Ok(Episode {
        id: row.try_get("id")?,
        session_id: row.try_get("session_id")?,
        cli: parse_enum(&cli_raw, "episodes.cli")?,
        model: row.try_get("model")?,
        os: parse_enum(&os_raw, "episodes.os")?,
        shell: parse_enum(&shell_raw, "episodes.shell")?,
        tool_name: row.try_get("tool_name")?,
        phase: parse_enum(&phase_raw, "episodes.phase")?,
        pattern_id: row.try_get("pattern_id")?,
        injected_fix_id: row.try_get("injected_fix_id")?,
        command_scrubbed: row.try_get("command_scrubbed")?,
        created_at: row.try_get("created_at")?,
    })
}

const SQL_UPSERT_PATTERN: &str = "INSERT INTO error_patterns \
    (id, signature_hash, title, error_template, tool_name, scope, repo_hash, occurrence_count, \
     first_seen_at, last_seen_at, embedding_status, updated_at) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
    ON CONFLICT (signature_hash) DO UPDATE SET \
      title = CASE WHEN EXCLUDED.title <> '' THEN EXCLUDED.title ELSE error_patterns.title END, \
      occurrence_count = error_patterns.occurrence_count + EXCLUDED.occurrence_count, \
      last_seen_at = GREATEST(error_patterns.last_seen_at, EXCLUDED.last_seen_at), \
      updated_at = GREATEST(error_patterns.updated_at, EXCLUDED.updated_at) \
    RETURNING id, signature_hash, title, error_template, tool_name, scope, repo_hash, \
      occurrence_count, first_seen_at, last_seen_at, embedding_status, updated_at";

const SQL_GET_PATTERN: &str = "SELECT id, signature_hash, title, error_template, tool_name, scope, \
    repo_hash, occurrence_count, first_seen_at, last_seen_at, embedding_status, updated_at \
    FROM error_patterns WHERE id = $1";

const SQL_FIND_PATTERN_SIG: &str = "SELECT id, signature_hash, title, error_template, tool_name, scope, \
    repo_hash, occurrence_count, first_seen_at, last_seen_at, embedding_status, updated_at \
    FROM error_patterns WHERE signature_hash = $1";

const SQL_LIST_PATTERNS: &str = "SELECT id, signature_hash, title, error_template, tool_name, scope, \
    repo_hash, occurrence_count, first_seen_at, last_seen_at, embedding_status, updated_at \
    FROM error_patterns \
    WHERE ($1::text IS NULL OR scope = $1) AND ($2::text IS NULL OR repo_hash = $2) \
    ORDER BY occurrence_count DESC, last_seen_at DESC LIMIT $3";

const SQL_LIST_PENDING_EMB: &str = "SELECT id, signature_hash, title, error_template, tool_name, scope, \
    repo_hash, occurrence_count, first_seen_at, last_seen_at, embedding_status, updated_at \
    FROM error_patterns WHERE embedding_status = 'pending' ORDER BY updated_at ASC LIMIT $1";

const SQL_UPSERT_FIX: &str = "INSERT INTO fixes \
    (id, summary, fix_type, pattern_before, pattern_after, guard_regex, status, scope, source, created_at, updated_at) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
    ON CONFLICT (id) DO UPDATE SET \
      summary = EXCLUDED.summary, fix_type = EXCLUDED.fix_type, \
      pattern_before = EXCLUDED.pattern_before, pattern_after = EXCLUDED.pattern_after, \
      guard_regex = EXCLUDED.guard_regex, status = EXCLUDED.status, scope = EXCLUDED.scope, \
      source = EXCLUDED.source, updated_at = EXCLUDED.updated_at \
    RETURNING id, summary, fix_type, pattern_before, pattern_after, guard_regex, status, scope, source, created_at, updated_at";

const SQL_GET_FIX: &str = "SELECT id, summary, fix_type, pattern_before, pattern_after, guard_regex, \
    status, scope, source, created_at, updated_at FROM fixes WHERE id = $1";

const SQL_LIST_PATTERNS_SINCE: &str = "SELECT id, signature_hash, title, error_template, tool_name, scope, \
    repo_hash, occurrence_count, first_seen_at, last_seen_at, embedding_status, updated_at \
    FROM error_patterns \
    WHERE updated_at > $1 AND scope = ANY($2) \
      AND (scope <> 'repo' OR ($3::text IS NOT NULL AND repo_hash = $3)) \
    ORDER BY updated_at ASC LIMIT $4";

const SQL_LIST_FIXES_SINCE: &str = "SELECT id, summary, fix_type, pattern_before, pattern_after, guard_regex, \
    status, scope, source, created_at, updated_at FROM fixes \
    WHERE updated_at > $1 AND scope = ANY($2) ORDER BY updated_at ASC LIMIT $3";

const SQL_MERGE_PATTERN_DELTA: &str = "INSERT INTO error_patterns \
    (id, signature_hash, title, error_template, tool_name, scope, repo_hash, occurrence_count, \
     first_seen_at, last_seen_at, embedding_status, updated_at) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
    ON CONFLICT (signature_hash) DO UPDATE SET \
      occurrence_count = error_patterns.occurrence_count + $8, \
      last_seen_at = GREATEST(error_patterns.last_seen_at, EXCLUDED.last_seen_at), \
      updated_at = GREATEST(error_patterns.updated_at, EXCLUDED.updated_at), \
      scope = CASE WHEN EXCLUDED.scope = 'team' THEN 'team' ELSE error_patterns.scope END, \
      repo_hash = CASE WHEN EXCLUDED.scope = 'team' THEN NULL ELSE error_patterns.repo_hash END \
    RETURNING id, signature_hash, title, error_template, tool_name, scope, repo_hash, \
      occurrence_count, first_seen_at, last_seen_at, embedding_status, updated_at";

const SQL_MERGE_FIX_LWW: &str = "INSERT INTO fixes \
    (id, summary, fix_type, pattern_before, pattern_after, guard_regex, status, scope, source, created_at, updated_at) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
    ON CONFLICT (id) DO UPDATE SET \
      summary = CASE WHEN EXCLUDED.updated_at >= fixes.updated_at THEN EXCLUDED.summary ELSE fixes.summary END, \
      fix_type = CASE WHEN EXCLUDED.updated_at >= fixes.updated_at THEN EXCLUDED.fix_type ELSE fixes.fix_type END, \
      pattern_before = CASE WHEN EXCLUDED.updated_at >= fixes.updated_at THEN EXCLUDED.pattern_before ELSE fixes.pattern_before END, \
      pattern_after = CASE WHEN EXCLUDED.updated_at >= fixes.updated_at THEN EXCLUDED.pattern_after ELSE fixes.pattern_after END, \
      guard_regex = CASE WHEN EXCLUDED.updated_at >= fixes.updated_at THEN EXCLUDED.guard_regex ELSE fixes.guard_regex END, \
      status = CASE WHEN EXCLUDED.updated_at >= fixes.updated_at THEN EXCLUDED.status ELSE fixes.status END, \
      scope = CASE WHEN EXCLUDED.updated_at >= fixes.updated_at THEN EXCLUDED.scope ELSE fixes.scope END, \
      source = CASE WHEN EXCLUDED.updated_at >= fixes.updated_at THEN EXCLUDED.source ELSE fixes.source END, \
      updated_at = GREATEST(fixes.updated_at, EXCLUDED.updated_at) \
    RETURNING id, summary, fix_type, pattern_before, pattern_after, guard_regex, status, scope, source, created_at, updated_at";

#[async_trait]
impl KnowledgeStore for PgStore {
    fn embedding_dims(&self) -> usize {
        self.embedding_dims
    }

    async fn upsert_pattern(&self, pattern: &ErrorPattern) -> Result<ErrorPattern, StoreError> {
        let delta = u64_to_i64(pattern.occurrence_count, "occurrence_count")?;
        let row = sqlx::query(SQL_UPSERT_PATTERN)
            .bind(&pattern.id)
            .bind(&pattern.signature_hash)
            .bind(&pattern.title)
            .bind(&pattern.error_template)
            .bind(&pattern.tool_name)
            .bind(pattern.scope.as_str())
            .bind(&pattern.repo_hash)
            .bind(delta)
            .bind(&pattern.first_seen_at)
            .bind(&pattern.last_seen_at)
            .bind(pattern.embedding_status.as_str())
            .bind(&pattern.updated_at)
            .fetch_one(&self.pool)
            .await?;
        row_to_pattern(&row)
    }

    async fn get_pattern(&self, id: &str) -> Result<Option<ErrorPattern>, StoreError> {
        let row = sqlx::query(SQL_GET_PATTERN)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_pattern).transpose()
    }

    async fn find_pattern_by_signature(
        &self,
        signature_hash: &str,
    ) -> Result<Option<ErrorPattern>, StoreError> {
        let row = sqlx::query(SQL_FIND_PATTERN_SIG)
            .bind(signature_hash)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_pattern).transpose()
    }

    async fn list_patterns(
        &self,
        scope: Option<Scope>,
        repo_hash: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ErrorPattern>, StoreError> {
        let rows = sqlx::query(SQL_LIST_PATTERNS)
            .bind(scope.map(Scope::as_str))
            .bind(repo_hash)
            .bind(usize_to_i64(limit, "limit")?)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_pattern).collect()
    }

    async fn update_pattern_scope(
        &self,
        id: &str,
        scope: Scope,
        repo_hash: Option<&str>,
        updated_at: &str,
    ) -> Result<(), StoreError> {
        let res = sqlx::query(
            "UPDATE error_patterns SET scope = $2, repo_hash = $3, updated_at = $4 WHERE id = $1",
        )
        .bind(id)
        .bind(scope.as_str())
        .bind(repo_hash)
        .bind(updated_at)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("error_pattern:{id}")));
        }
        Ok(())
    }

    async fn list_pending_embeddings(&self, limit: usize) -> Result<Vec<ErrorPattern>, StoreError> {
        let rows = sqlx::query(SQL_LIST_PENDING_EMB)
            .bind(usize_to_i64(limit, "limit")?)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_pattern).collect()
    }

    async fn upsert_pattern_embedding(
        &self,
        pattern_id: &str,
        embedding: &[f32],
    ) -> Result<(), StoreError> {
        if embedding.len() != self.embedding_dims {
            return Err(StoreError::DimensionMismatch {
                expected: self.embedding_dims,
                actual: embedding.len(),
            });
        }
        let vec_literal = encode_pgvector(embedding);
        let res = sqlx::query(
            "UPDATE error_patterns \
             SET embedding = $2::vector, embedding_status = $3 \
             WHERE id = $1",
        )
        .bind(pattern_id)
        .bind(vec_literal)
        .bind(EmbeddingStatus::Ready.as_str())
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("error_pattern:{pattern_id}")));
        }
        Ok(())
    }

    async fn search_bm25(
        &self,
        query: &str,
        tool_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Bm25Hit>, StoreError> {
        let trimmed = query.trim();
        if trimmed.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "WITH q AS (SELECT plainto_tsquery('english', $1) AS tsq) \
             SELECT p.id, \
                    (ts_rank_cd(p.search_tsv, q.tsq)::float8 + similarity(p.error_template, $1)::float8 * 0.5) AS score \
             FROM error_patterns p, q \
             WHERE ($2::text IS NULL OR p.tool_name = $2) \
               AND (p.search_tsv @@ q.tsq OR p.error_template % $1) \
             ORDER BY score DESC \
             LIMIT $3",
        )
        .bind(trimmed)
        .bind(tool_name)
        .bind(usize_to_i64(limit, "limit")?)
        .fetch_all(&self.pool)
        .await?;

        let mut hits = Vec::with_capacity(rows.len());
        for (idx, row) in rows.iter().enumerate() {
            hits.push(Bm25Hit {
                pattern_id: row.try_get("id")?,
                score: row.try_get("score")?,
                rank: idx + 1,
            });
        }
        Ok(hits)
    }

    async fn search_vector(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorHit>, StoreError> {
        if embedding.len() != self.embedding_dims {
            return Err(StoreError::DimensionMismatch {
                expected: self.embedding_dims,
                actual: embedding.len(),
            });
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let vec_literal = encode_pgvector(embedding);
        let rows = sqlx::query(
            "SELECT id, (embedding <=> $1::vector)::float8 AS distance \
             FROM error_patterns \
             WHERE embedding IS NOT NULL \
             ORDER BY embedding <=> $1::vector ASC \
             LIMIT $2",
        )
        .bind(vec_literal)
        .bind(usize_to_i64(limit, "limit")?)
        .fetch_all(&self.pool)
        .await?;

        let mut hits = Vec::with_capacity(rows.len());
        for (idx, row) in rows.iter().enumerate() {
            hits.push(VectorHit {
                pattern_id: row.try_get("id")?,
                distance: row.try_get("distance")?,
                rank: idx + 1,
            });
        }
        Ok(hits)
    }

    async fn upsert_fix(&self, fix: &Fix) -> Result<Fix, StoreError> {
        let row = sqlx::query(SQL_UPSERT_FIX)
            .bind(&fix.id)
            .bind(&fix.summary)
            .bind(fix.fix_type.as_str())
            .bind(&fix.pattern_before)
            .bind(&fix.pattern_after)
            .bind(&fix.guard_regex)
            .bind(fix.status.as_str())
            .bind(fix.scope.as_str())
            .bind(fix.source.as_str())
            .bind(&fix.created_at)
            .bind(&fix.updated_at)
            .fetch_one(&self.pool)
            .await?;
        row_to_fix(&row)
    }

    async fn get_fix(&self, id: &str) -> Result<Option<Fix>, StoreError> {
        let row = sqlx::query(SQL_GET_FIX)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_fix).transpose()
    }

    async fn update_fix_status(
        &self,
        id: &str,
        status: FixStatus,
        updated_at: &str,
    ) -> Result<(), StoreError> {
        let res = sqlx::query("UPDATE fixes SET status = $2, updated_at = $3 WHERE id = $1")
            .bind(id)
            .bind(status.as_str())
            .bind(updated_at)
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("fix:{id}")));
        }
        Ok(())
    }

    async fn list_active_guards(&self, repo_hash: Option<&str>) -> Result<Vec<Fix>, StoreError> {
        let rows = sqlx::query(
            "SELECT DISTINCT f.id, f.summary, f.fix_type, f.pattern_before, f.pattern_after, \
                             f.guard_regex, f.status, f.scope, f.source, f.created_at, f.updated_at \
             FROM fixes f \
             WHERE f.fix_type = 'pre_tool_guard' \
               AND f.status IN ('verified', 'candidate') \
               AND f.guard_regex IS NOT NULL \
               AND ( \
                   f.scope <> 'repo' \
                   OR ( \
                       $1::text IS NOT NULL \
                       AND EXISTS ( \
                           SELECT 1 \
                           FROM graph_edges e \
                           JOIN error_patterns p ON e.src_id = ('err:' || p.id) \
                           WHERE e.edge_type = 'RESOLVED_BY' \
                             AND e.dst_id = ('fix:' || f.id) \
                             AND p.repo_hash = $1 \
                       ) \
                   ) \
               ) \
             ORDER BY f.updated_at DESC",
        )
        .bind(repo_hash)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_fix).collect()
    }

    async fn upsert_node(&self, node: &GraphNode) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO graph_nodes (id, kind, label, props_json) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (id) DO UPDATE SET \
               kind = EXCLUDED.kind, \
               label = EXCLUDED.label, \
               props_json = EXCLUDED.props_json",
        )
        .bind(&node.id)
        .bind(node.kind.as_str())
        .bind(&node.label)
        .bind(&node.props_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn upsert_edge(&self, edge: &GraphEdge) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO graph_edges \
             (src_id, edge_type, dst_id, worked_count, failed_count, hit_count, last_seen_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (src_id, edge_type, dst_id) DO UPDATE SET \
               worked_count = EXCLUDED.worked_count, \
               failed_count = EXCLUDED.failed_count, \
               hit_count = EXCLUDED.hit_count, \
               last_seen_at = EXCLUDED.last_seen_at",
        )
        .bind(&edge.src_id)
        .bind(edge.edge_type.as_str())
        .bind(&edge.dst_id)
        .bind(u64_to_i64(edge.worked_count, "worked_count")?)
        .bind(u64_to_i64(edge.failed_count, "failed_count")?)
        .bind(u64_to_i64(edge.hit_count, "hit_count")?)
        .bind(&edge.last_seen_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_edge_feedback(
        &self,
        src_id: &str,
        edge_type: EdgeType,
        dst_id: &str,
        worked_delta: u64,
        failed_delta: u64,
        hit_delta: u64,
        last_seen_at: &str,
    ) -> Result<GraphEdge, StoreError> {
        let row = sqlx::query(
            "INSERT INTO graph_edges \
             (src_id, edge_type, dst_id, worked_count, failed_count, hit_count, last_seen_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (src_id, edge_type, dst_id) DO UPDATE SET \
               worked_count = graph_edges.worked_count + EXCLUDED.worked_count, \
               failed_count = graph_edges.failed_count + EXCLUDED.failed_count, \
               hit_count = graph_edges.hit_count + EXCLUDED.hit_count, \
               last_seen_at = GREATEST(graph_edges.last_seen_at, EXCLUDED.last_seen_at) \
             RETURNING src_id, edge_type, dst_id, worked_count, failed_count, hit_count, last_seen_at",
        )
        .bind(src_id)
        .bind(edge_type.as_str())
        .bind(dst_id)
        .bind(u64_to_i64(worked_delta, "worked_delta")?)
        .bind(u64_to_i64(failed_delta, "failed_delta")?)
        .bind(u64_to_i64(hit_delta, "hit_delta")?)
        .bind(last_seen_at)
        .fetch_one(&self.pool)
        .await?;
        row_to_edge(&row)
    }

    async fn neighbors(
        &self,
        node_id: &str,
        edge_types: Option<&[EdgeType]>,
        max_hops: u8,
    ) -> Result<Vec<Neighbor>, StoreError> {
        if max_hops == 0 {
            return Ok(Vec::new());
        }
        let max_hops = max_hops.min(2);
        let filter: Option<HashSet<EdgeType>> = edge_types.map(|s| s.iter().copied().collect());
        let mut results = Vec::new();
        let mut visited = HashSet::from([node_id.to_owned()]);
        let mut seen_edges = HashSet::new();
        let mut frontier = VecDeque::from([(node_id.to_owned(), 0_u8)]);

        while let Some((current, depth)) = frontier.pop_front() {
            if depth >= max_hops {
                continue;
            }
            let next_hop = depth + 1;
            let out_rows = sqlx::query(
                "SELECT n.id, n.kind, n.label, n.props_json, \
                        e.src_id, e.edge_type, e.dst_id, e.worked_count, e.failed_count, e.hit_count, e.last_seen_at \
                 FROM graph_edges e \
                 JOIN graph_nodes n ON n.id = e.dst_id \
                 WHERE e.src_id = $1",
            )
            .bind(&current)
            .fetch_all(&self.pool)
            .await?;

            let in_rows = sqlx::query(
                "SELECT n.id, n.kind, n.label, n.props_json, \
                        e.src_id, e.edge_type, e.dst_id, e.worked_count, e.failed_count, e.hit_count, e.last_seen_at \
                 FROM graph_edges e \
                 JOIN graph_nodes n ON n.id = e.src_id \
                 WHERE e.dst_id = $1",
            )
            .bind(&current)
            .fetch_all(&self.pool)
            .await?;

            for (direction, rows) in [
                (EdgeDirection::Outgoing, out_rows),
                (EdgeDirection::Incoming, in_rows),
            ] {
                for row in &rows {
                    let node = row_to_node(row)?;
                    let edge = row_to_edge(row)?;
                    if filter
                        .as_ref()
                        .is_some_and(|set| !set.contains(&edge.edge_type))
                    {
                        continue;
                    }
                    let key = (edge.src_id.clone(), edge.edge_type, edge.dst_id.clone());
                    if !seen_edges.insert(key) {
                        continue;
                    }
                    if visited.insert(node.id.clone()) && next_hop < max_hops {
                        frontier.push_back((node.id.clone(), next_hop));
                    }
                    results.push(Neighbor {
                        node,
                        edge,
                        direction,
                        hop: next_hop,
                    });
                }
            }
        }
        Ok(results)
    }

    async fn fixes_for_pattern(&self, pattern_id: &str) -> Result<Vec<FixCandidate>, StoreError> {
        let src_id = format!("err:{pattern_id}");
        let rows = sqlx::query(
            "SELECT f.id, f.summary, f.fix_type, f.pattern_before, f.pattern_after, \
                    f.guard_regex, f.status, f.scope, f.source, f.created_at, f.updated_at, \
                    e.src_id, e.edge_type, e.dst_id, e.worked_count, e.failed_count, e.hit_count, e.last_seen_at \
             FROM graph_edges e \
             JOIN fixes f ON ('fix:' || f.id) = e.dst_id \
             WHERE e.src_id = $1 AND e.edge_type = 'RESOLVED_BY' \
             ORDER BY e.worked_count DESC, e.last_seen_at DESC",
        )
        .bind(src_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(FixCandidate {
                fix: row_to_fix(row)?,
                edge: row_to_edge(row)?,
            });
        }
        Ok(out)
    }

    async fn pattern_repo_count(&self, pattern_id: &str) -> Result<usize, StoreError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT e.dst_id) \
             FROM graph_edges e \
             JOIN graph_nodes n ON n.id = e.dst_id \
             WHERE e.src_id = $1 AND e.edge_type = 'OCCURS_IN' AND n.kind = 'repo'",
        )
        .bind(format!("err:{pattern_id}"))
        .fetch_one(&self.pool)
        .await?;
        usize::try_from(count).map_err(|err| StoreError::InvalidData {
            context: "pattern_repo_count",
            detail: err.to_string(),
        })
    }

    async fn pattern_contexts(&self, pattern_id: &str) -> Result<Vec<GraphNode>, StoreError> {
        let rows = sqlx::query(
            "SELECT n.id, n.kind, n.label, n.props_json \
             FROM graph_edges e \
             JOIN graph_nodes n ON n.id = e.dst_id \
             WHERE e.src_id = $1 AND e.edge_type = 'OCCURS_IN'",
        )
        .bind(format!("err:{pattern_id}"))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_node).collect()
    }

    async fn is_fix_superseded_or_obsolete(
        &self,
        fix_id: &str,
        cli: Option<&str>,
        model: Option<&str>,
    ) -> Result<bool, StoreError> {
        let flagged: bool = sqlx::query_scalar(
            "SELECT EXISTS ( \
                SELECT 1 FROM graph_edges \
                WHERE src_id = $1 \
                  AND ( \
                      edge_type = 'SUPERSEDED_BY' \
                      OR (edge_type = 'OBSOLETE_IN' AND (dst_id = $2 OR dst_id = $3)) \
                  ) \
             )",
        )
        .bind(format!("fix:{fix_id}"))
        .bind(cli.map(|c| format!("cli:{c}")))
        .bind(model.map(|m| format!("model:{m}")))
        .fetch_one(&self.pool)
        .await?;
        Ok(flagged)
    }

    async fn record_episode(&self, ep: &Episode) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO episodes \
             (id, session_id, cli, model, os, shell, tool_name, phase, \
              pattern_id, injected_fix_id, command_scrubbed, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(&ep.id)
        .bind(&ep.session_id)
        .bind(ep.cli.as_str())
        .bind(&ep.model)
        .bind(ep.os.as_str())
        .bind(ep.shell.as_str())
        .bind(&ep.tool_name)
        .bind(ep.phase.as_str())
        .bind(&ep.pattern_id)
        .bind(&ep.injected_fix_id)
        .bind(&ep.command_scrubbed)
        .bind(&ep.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn recent_session_episodes(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<Episode>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, session_id, cli, model, os, shell, tool_name, phase, \
                    pattern_id, injected_fix_id, command_scrubbed, created_at \
             FROM episodes \
             WHERE session_id = $1 \
             ORDER BY created_at DESC \
             LIMIT $2",
        )
        .bind(session_id)
        .bind(usize_to_i64(limit, "limit")?)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_episode).collect()
    }

    async fn prune_episodes_before(&self, cutoff_iso: &str) -> Result<usize, StoreError> {
        let res = sqlx::query("DELETE FROM episodes WHERE created_at < $1")
            .bind(cutoff_iso)
            .execute(&self.pool)
            .await?;
        usize::try_from(res.rows_affected()).map_err(|err| StoreError::InvalidData {
            context: "prune_episodes_before",
            detail: err.to_string(),
        })
    }

    async fn enqueue_outbox(
        &self,
        entity_type: &str,
        entity_id: &str,
        payload_json: &str,
        created_at: &str,
    ) -> Result<i64, StoreError> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO sync_outbox (entity_type, entity_id, payload_json, created_at) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(entity_type)
        .bind(entity_id)
        .bind(payload_json)
        .bind(created_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    async fn list_outbox(&self, limit: usize) -> Result<Vec<OutboxEntry>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, entity_type, entity_id, payload_json, created_at \
             FROM sync_outbox ORDER BY id ASC LIMIT $1",
        )
        .bind(usize_to_i64(limit, "limit")?)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(OutboxEntry {
                id: row.try_get("id")?,
                entity_type: row.try_get("entity_type")?,
                entity_id: row.try_get("entity_id")?,
                payload_json: row.try_get("payload_json")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    async fn delete_outbox_entries(&self, ids: &[i64]) -> Result<usize, StoreError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let res = sqlx::query("DELETE FROM sync_outbox WHERE id = ANY($1)")
            .bind(ids)
            .execute(&self.pool)
            .await?;
        usize::try_from(res.rows_affected()).map_err(|err| StoreError::InvalidData {
            context: "delete_outbox_entries",
            detail: err.to_string(),
        })
    }

    async fn list_patterns_updated_since(
        &self,
        since_iso: &str,
        scopes: &[Scope],
        repo_hash: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ErrorPattern>, StoreError> {
        let scope_list: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
        let rows = sqlx::query(SQL_LIST_PATTERNS_SINCE)
            .bind(since_iso)
            .bind(&scope_list)
            .bind(repo_hash)
            .bind(usize_to_i64(limit, "limit")?)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_pattern).collect()
    }

    async fn list_fixes_updated_since(
        &self,
        since_iso: &str,
        scopes: &[Scope],
        limit: usize,
    ) -> Result<Vec<Fix>, StoreError> {
        let scope_list: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
        let rows = sqlx::query(SQL_LIST_FIXES_SINCE)
            .bind(since_iso)
            .bind(&scope_list)
            .bind(usize_to_i64(limit, "limit")?)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_fix).collect()
    }

    async fn list_edges_updated_since(
        &self,
        since_iso: &str,
        limit: usize,
    ) -> Result<Vec<GraphEdge>, StoreError> {
        let rows = sqlx::query(
            "SELECT src_id, edge_type, dst_id, worked_count, failed_count, hit_count, last_seen_at \
             FROM graph_edges \
             WHERE last_seen_at > $1 \
             ORDER BY last_seen_at ASC \
             LIMIT $2",
        )
        .bind(since_iso)
        .bind(usize_to_i64(limit, "limit")?)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_edge).collect()
    }

    async fn list_nodes_by_ids(&self, node_ids: &[String]) -> Result<Vec<GraphNode>, StoreError> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows =
            sqlx::query("SELECT id, kind, label, props_json FROM graph_nodes WHERE id = ANY($1)")
                .bind(node_ids)
                .fetch_all(&self.pool)
                .await?;
        rows.iter().map(row_to_node).collect()
    }

    async fn merge_pattern_delta(
        &self,
        pattern: &ErrorPattern,
        occurrence_delta: u64,
    ) -> Result<ErrorPattern, StoreError> {
        let delta = u64_to_i64(occurrence_delta, "occurrence_delta")?;
        let row = sqlx::query(SQL_MERGE_PATTERN_DELTA)
            .bind(&pattern.id)
            .bind(&pattern.signature_hash)
            .bind(&pattern.title)
            .bind(&pattern.error_template)
            .bind(&pattern.tool_name)
            .bind(pattern.scope.as_str())
            .bind(&pattern.repo_hash)
            .bind(delta)
            .bind(&pattern.first_seen_at)
            .bind(&pattern.last_seen_at)
            .bind(pattern.embedding_status.as_str())
            .bind(&pattern.updated_at)
            .fetch_one(&self.pool)
            .await?;
        row_to_pattern(&row)
    }

    async fn merge_fix_lww(&self, fix: &Fix) -> Result<Fix, StoreError> {
        let row = sqlx::query(SQL_MERGE_FIX_LWW)
            .bind(&fix.id)
            .bind(&fix.summary)
            .bind(fix.fix_type.as_str())
            .bind(&fix.pattern_before)
            .bind(&fix.pattern_after)
            .bind(&fix.guard_regex)
            .bind(fix.status.as_str())
            .bind(fix.scope.as_str())
            .bind(fix.source.as_str())
            .bind(&fix.created_at)
            .bind(&fix.updated_at)
            .fetch_one(&self.pool)
            .await?;
        row_to_fix(&row)
    }
}
