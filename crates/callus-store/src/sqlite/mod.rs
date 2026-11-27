//! Embedded SQLite (WAL) implementation of [`KnowledgeStore`].
//!
//! Combines `FTS5` (BM25), `sqlite-vec` (`vec0` cosine KNN), and indexed
//! relational graph tables (`graph_nodes`, `graph_edges`) in a single file.

pub mod migrations;
mod vec_ext;

use std::{
    collections::{HashSet, VecDeque},
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use callus_core::{
    EdgeType, EmbeddingStatus, Episode, ErrorPattern, Fix, FixStatus, GraphEdge, GraphNode, Scope,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

pub use migrations::DEFAULT_EMBEDDING_DIMS;
use migrations::apply_migrations;
use vec_ext::{encode_f32_le, register_sqlite_vec};

use crate::{
    Bm25Hit, EdgeDirection, FixCandidate, KnowledgeStore, Neighbor, OutboxEntry, StoreError,
    VectorHit,
};

/// Local embedded SQLite store using WAL, FTS5, `sqlite-vec`, and graph tables.
#[derive(Debug, Clone)]
pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
    embedding_dims: usize,
}

impl SqliteStore {
    /// Opens (or creates) a file-backed SQLite database in WAL mode.
    pub fn open(path: impl AsRef<Path>, embedding_dims: usize) -> Result<Self, StoreError> {
        register_sqlite_vec();
        let conn = Connection::open(path)?;
        apply_migrations(&conn, embedding_dims, true)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            embedding_dims,
        })
    }

    /// Opens an isolated in-memory SQLite database (primarily for tests).
    pub fn open_in_memory(embedding_dims: usize) -> Result<Self, StoreError> {
        register_sqlite_vec();
        let conn = Connection::open_in_memory()?;
        apply_migrations(&conn, embedding_dims, false)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            embedding_dims,
        })
    }

    /// Returns the active SQLite `journal_mode` pragma (e.g. `"wal"`).
    pub fn journal_mode(&self) -> Result<String, StoreError> {
        let guard = self.lock_conn()?;
        let mode: String = guard.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        Ok(mode)
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.conn
            .lock()
            .map_err(|err| StoreError::TaskJoin(format!("sqlite mutex poisoned: {err}")))
    }

    async fn run<T, F>(&self, op: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|err| StoreError::TaskJoin(format!("sqlite mutex poisoned: {err}")))?;
            op(&mut guard)
        })
        .await
        .map_err(|err| StoreError::TaskJoin(err.to_string()))?
    }
}

/// Converts arbitrary error text into a safe SQLite FTS5 `MATCH` expression.
pub fn sanitize_fts_query(raw: &str) -> Option<String> {
    let mut seen = HashSet::new();
    let mut tokens = Vec::new();
    for piece in raw.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if piece.len() < 2 {
            continue;
        }
        let lower = piece.to_ascii_lowercase();
        if seen.insert(lower.clone()) {
            tokens.push(format!("\"{lower}\""));
            if tokens.len() == 16 {
                break;
            }
        }
    }
    if tokens.is_empty() {
        for piece in raw.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if !piece.is_empty() {
                tokens.push(format!("\"{}\"", piece.to_ascii_lowercase()));
                break;
            }
        }
    }
    (!tokens.is_empty()).then(|| tokens.join(" OR "))
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

fn row_to_pattern(row: &Row<'_>) -> Result<ErrorPattern, StoreError> {
    let scope_raw: String = row.get(5)?;
    let count_raw: i64 = row.get(7)?;
    let status_raw: String = row.get(10)?;
    Ok(ErrorPattern {
        id: row.get(0)?,
        signature_hash: row.get(1)?,
        title: row.get(2)?,
        error_template: row.get(3)?,
        tool_name: row.get(4)?,
        scope: parse_enum(&scope_raw, "error_patterns.scope")?,
        repo_hash: row.get(6)?,
        occurrence_count: i64_to_u64(count_raw, "error_patterns.occurrence_count")?,
        first_seen_at: row.get(8)?,
        last_seen_at: row.get(9)?,
        embedding_status: parse_enum(&status_raw, "error_patterns.embedding_status")?,
        updated_at: row.get(11)?,
    })
}

fn row_to_fix(row: &Row<'_>) -> Result<Fix, StoreError> {
    let fix_type_raw: String = row.get(2)?;
    let status_raw: String = row.get(6)?;
    let scope_raw: String = row.get(7)?;
    let source_raw: String = row.get(8)?;
    Ok(Fix {
        id: row.get(0)?,
        summary: row.get(1)?,
        fix_type: parse_enum(&fix_type_raw, "fixes.fix_type")?,
        pattern_before: row.get(3)?,
        pattern_after: row.get(4)?,
        guard_regex: row.get(5)?,
        status: parse_enum(&status_raw, "fixes.status")?,
        scope: parse_enum(&scope_raw, "fixes.scope")?,
        source: parse_enum(&source_raw, "fixes.source")?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn row_to_node(row: &Row<'_>) -> Result<GraphNode, StoreError> {
    let kind_raw: String = row.get(1)?;
    let props_raw: String = row.get(3)?;
    Ok(GraphNode {
        id: row.get(0)?,
        kind: parse_enum(&kind_raw, "graph_nodes.kind")?,
        label: row.get(2)?,
        props_json: serde_json::from_str(&props_raw)?,
    })
}

fn row_to_edge(row: &Row<'_>) -> Result<GraphEdge, StoreError> {
    let edge_type_raw: String = row.get(1)?;
    let worked_raw: i64 = row.get(3)?;
    let failed_raw: i64 = row.get(4)?;
    let hit_raw: i64 = row.get(5)?;
    Ok(GraphEdge {
        src_id: row.get(0)?,
        edge_type: parse_enum(&edge_type_raw, "graph_edges.edge_type")?,
        dst_id: row.get(2)?,
        worked_count: i64_to_u64(worked_raw, "graph_edges.worked_count")?,
        failed_count: i64_to_u64(failed_raw, "graph_edges.failed_count")?,
        hit_count: i64_to_u64(hit_raw, "graph_edges.hit_count")?,
        last_seen_at: row.get(6)?,
    })
}

fn row_to_episode(row: &Row<'_>) -> Result<Episode, StoreError> {
    let cli_raw: String = row.get(2)?;
    let os_raw: String = row.get(4)?;
    let shell_raw: String = row.get(5)?;
    let phase_raw: String = row.get(7)?;
    Ok(Episode {
        id: row.get(0)?,
        session_id: row.get(1)?,
        cli: parse_enum(&cli_raw, "episodes.cli")?,
        model: row.get(3)?,
        os: parse_enum(&os_raw, "episodes.os")?,
        shell: parse_enum(&shell_raw, "episodes.shell")?,
        tool_name: row.get(6)?,
        phase: parse_enum(&phase_raw, "episodes.phase")?,
        pattern_id: row.get(8)?,
        injected_fix_id: row.get(9)?,
        command_scrubbed: row.get(10)?,
        created_at: row.get(11)?,
    })
}

fn sync_fts(tx: &rusqlite::Transaction<'_>, pattern: &ErrorPattern) -> Result<(), StoreError> {
    tx.execute(
        "DELETE FROM error_patterns_fts WHERE pattern_id = ?1",
        params![pattern.id],
    )?;
    tx.execute(
        "INSERT INTO error_patterns_fts (pattern_id, title, error_template, tool_name) \
         VALUES (?1, ?2, ?3, ?4)",
        params![
            pattern.id,
            pattern.title,
            pattern.error_template,
            pattern.tool_name
        ],
    )?;
    Ok(())
}

const PATTERN_COLS: &str = "id, signature_hash, title, error_template, tool_name, scope, \
                            repo_hash, occurrence_count, first_seen_at, last_seen_at, \
                            embedding_status, updated_at";

const FIX_COLS: &str = "id, summary, fix_type, pattern_before, pattern_after, guard_regex, \
                        status, scope, source, created_at, updated_at";

#[async_trait]
impl KnowledgeStore for SqliteStore {
    fn embedding_dims(&self) -> usize {
        self.embedding_dims
    }

    async fn upsert_pattern(&self, pattern: &ErrorPattern) -> Result<ErrorPattern, StoreError> {
        let pattern = pattern.clone();
        self.run(move |conn| {
            let tx = conn.transaction()?;
            let existing = tx
                .query_row(
                    &format!("SELECT {PATTERN_COLS} FROM error_patterns WHERE signature_hash = ?1"),
                    params![pattern.signature_hash],
                    |row| Ok(row_to_pattern(row)),
                )
                .optional()?
                .transpose()?;

            let stored = if let Some(mut cur) = existing {
                cur.occurrence_count = cur
                    .occurrence_count
                    .saturating_add(pattern.occurrence_count.max(1));
                if !pattern.title.is_empty() {
                    cur.title = pattern.title;
                }
                cur.last_seen_at = pattern.last_seen_at;
                cur.updated_at = pattern.updated_at;
                tx.execute(
                    "UPDATE error_patterns \
                     SET title = ?2, occurrence_count = ?3, last_seen_at = ?4, updated_at = ?5 \
                     WHERE id = ?1",
                    params![
                        cur.id,
                        cur.title,
                        u64_to_i64(cur.occurrence_count, "occurrence_count")?,
                        cur.last_seen_at,
                        cur.updated_at,
                    ],
                )?;
                cur
            } else {
                tx.execute(
                    &format!(
                        "INSERT INTO error_patterns ({PATTERN_COLS}) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                    ),
                    params![
                        pattern.id,
                        pattern.signature_hash,
                        pattern.title,
                        pattern.error_template,
                        pattern.tool_name,
                        pattern.scope.as_str(),
                        pattern.repo_hash,
                        u64_to_i64(pattern.occurrence_count.max(1), "occurrence_count")?,
                        pattern.first_seen_at,
                        pattern.last_seen_at,
                        pattern.embedding_status.as_str(),
                        pattern.updated_at,
                    ],
                )?;
                pattern
            };

            sync_fts(&tx, &stored)?;
            tx.commit()?;
            Ok(stored)
        })
        .await
    }

    async fn get_pattern(&self, id: &str) -> Result<Option<ErrorPattern>, StoreError> {
        let id = id.to_owned();
        self.run(move |conn| {
            conn.query_row(
                &format!("SELECT {PATTERN_COLS} FROM error_patterns WHERE id = ?1"),
                params![id],
                |row| Ok(row_to_pattern(row)),
            )
            .optional()?
            .transpose()
        })
        .await
    }

    async fn find_pattern_by_signature(
        &self,
        signature_hash: &str,
    ) -> Result<Option<ErrorPattern>, StoreError> {
        let sig = signature_hash.to_owned();
        self.run(move |conn| {
            conn.query_row(
                &format!("SELECT {PATTERN_COLS} FROM error_patterns WHERE signature_hash = ?1"),
                params![sig],
                |row| Ok(row_to_pattern(row)),
            )
            .optional()?
            .transpose()
        })
        .await
    }

    async fn list_patterns(
        &self,
        scope: Option<Scope>,
        repo_hash: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ErrorPattern>, StoreError> {
        let scope_str = scope.map(|s| s.as_str().to_owned());
        let repo = repo_hash.map(str::to_owned);
        self.run(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {PATTERN_COLS} FROM error_patterns \
                 WHERE (?1 IS NULL OR scope = ?1) \
                   AND (?2 IS NULL OR repo_hash = ?2) \
                 ORDER BY occurrence_count DESC, last_seen_at DESC \
                 LIMIT ?3"
            ))?;
            let rows = stmt.query_map(
                params![scope_str, repo, usize_to_i64(limit, "limit")?],
                |row| Ok(row_to_pattern(row)),
            )?;
            rows.map(|r| r?).collect()
        })
        .await
    }

    async fn update_pattern_scope(
        &self,
        id: &str,
        scope: Scope,
        repo_hash: Option<&str>,
        updated_at: &str,
    ) -> Result<(), StoreError> {
        let id = id.to_owned();
        let repo = repo_hash.map(str::to_owned);
        let updated_at = updated_at.to_owned();
        self.run(move |conn| {
            let changed = conn.execute(
                "UPDATE error_patterns SET scope = ?2, repo_hash = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, scope.as_str(), repo, updated_at],
            )?;
            if changed == 0 {
                return Err(StoreError::NotFound(format!("error_pattern:{id}")));
            }
            Ok(())
        })
        .await
    }

    async fn list_pending_embeddings(&self, limit: usize) -> Result<Vec<ErrorPattern>, StoreError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {PATTERN_COLS} FROM error_patterns \
                 WHERE embedding_status = 'pending' \
                 ORDER BY updated_at ASC \
                 LIMIT ?1"
            ))?;
            let rows = stmt.query_map(params![usize_to_i64(limit, "limit")?], |row| {
                Ok(row_to_pattern(row))
            })?;
            rows.map(|r| r?).collect()
        })
        .await
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
        let pattern_id = pattern_id.to_owned();
        let bytes = encode_f32_le(embedding);
        self.run(move |conn| {
            let tx = conn.transaction()?;
            let updated = tx.execute(
                "UPDATE error_patterns SET embedding_status = ?2 WHERE id = ?1",
                params![pattern_id, EmbeddingStatus::Ready.as_str()],
            )?;
            if updated == 0 {
                return Err(StoreError::NotFound(format!("error_pattern:{pattern_id}")));
            }
            tx.execute(
                "DELETE FROM error_patterns_vec WHERE pattern_id = ?1",
                params![pattern_id],
            )?;
            tx.execute(
                "INSERT INTO error_patterns_vec (pattern_id, embedding) VALUES (?1, ?2)",
                params![pattern_id, bytes],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn search_bm25(
        &self,
        query: &str,
        tool_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Bm25Hit>, StoreError> {
        let Some(fts_query) = sanitize_fts_query(query) else {
            return Ok(Vec::new());
        };
        if limit == 0 {
            return Ok(Vec::new());
        }
        let tool = tool_name.map(str::to_owned);
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT f.pattern_id, -bm25(error_patterns_fts) AS score \
                 FROM error_patterns_fts f \
                 JOIN error_patterns p ON p.id = f.pattern_id \
                 WHERE error_patterns_fts MATCH ?1 \
                   AND (?2 IS NULL OR p.tool_name = ?2) \
                 ORDER BY score DESC \
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(
                params![fts_query, tool, usize_to_i64(limit, "limit")?],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
            )?;
            let mut hits = Vec::new();
            for (idx, item) in rows.enumerate() {
                let (pattern_id, score) = item?;
                hits.push(Bm25Hit {
                    pattern_id,
                    score,
                    rank: idx + 1,
                });
            }
            Ok(hits)
        })
        .await
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
        let bytes = encode_f32_le(embedding);
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT pattern_id, distance \
                 FROM error_patterns_vec \
                 WHERE embedding MATCH ?1 AND k = ?2 \
                 ORDER BY distance ASC",
            )?;
            let rows = stmt.query_map(params![bytes, usize_to_i64(limit, "limit")?], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })?;
            let mut hits = Vec::new();
            for (idx, item) in rows.enumerate() {
                let (pattern_id, distance) = item?;
                hits.push(VectorHit {
                    pattern_id,
                    distance,
                    rank: idx + 1,
                });
            }
            Ok(hits)
        })
        .await
    }

    async fn upsert_fix(&self, fix: &Fix) -> Result<Fix, StoreError> {
        let fix = fix.clone();
        self.run(move |conn| {
            conn.execute(
                &format!(
                    "INSERT INTO fixes ({FIX_COLS}) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
                     ON CONFLICT(id) DO UPDATE SET \
                       summary = excluded.summary, \
                       fix_type = excluded.fix_type, \
                       pattern_before = excluded.pattern_before, \
                       pattern_after = excluded.pattern_after, \
                       guard_regex = excluded.guard_regex, \
                       status = excluded.status, \
                       scope = excluded.scope, \
                       source = excluded.source, \
                       updated_at = excluded.updated_at"
                ),
                params![
                    fix.id,
                    fix.summary,
                    fix.fix_type.as_str(),
                    fix.pattern_before,
                    fix.pattern_after,
                    fix.guard_regex,
                    fix.status.as_str(),
                    fix.scope.as_str(),
                    fix.source.as_str(),
                    fix.created_at,
                    fix.updated_at,
                ],
            )?;
            Ok(fix)
        })
        .await
    }

    async fn get_fix(&self, id: &str) -> Result<Option<Fix>, StoreError> {
        let id = id.to_owned();
        self.run(move |conn| {
            conn.query_row(
                &format!("SELECT {FIX_COLS} FROM fixes WHERE id = ?1"),
                params![id],
                |row| Ok(row_to_fix(row)),
            )
            .optional()?
            .transpose()
        })
        .await
    }

    async fn update_fix_status(
        &self,
        id: &str,
        status: FixStatus,
        updated_at: &str,
    ) -> Result<(), StoreError> {
        let id = id.to_owned();
        let updated_at = updated_at.to_owned();
        self.run(move |conn| {
            let changed = conn.execute(
                "UPDATE fixes SET status = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, status.as_str(), updated_at],
            )?;
            if changed == 0 {
                return Err(StoreError::NotFound(format!("fix:{id}")));
            }
            Ok(())
        })
        .await
    }

    async fn list_active_guards(&self, repo_hash: Option<&str>) -> Result<Vec<Fix>, StoreError> {
        let repo = repo_hash.map(str::to_owned);
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT f.id, f.summary, f.fix_type, f.pattern_before, f.pattern_after, \
                                 f.guard_regex, f.status, f.scope, f.source, f.created_at, f.updated_at \
                 FROM fixes f \
                 WHERE f.fix_type = 'pre_tool_guard' \
                   AND f.status IN ('verified', 'candidate') \
                   AND f.guard_regex IS NOT NULL \
                   AND ( \
                       f.scope != 'repo' \
                       OR ( \
                           ?1 IS NOT NULL \
                           AND EXISTS ( \
                               SELECT 1 \
                               FROM graph_edges e \
                               JOIN error_patterns p ON e.src_id = ('err:' || p.id) \
                               WHERE e.edge_type = 'RESOLVED_BY' \
                                 AND e.dst_id = ('fix:' || f.id) \
                                 AND p.repo_hash = ?1 \
                           ) \
                       ) \
                   ) \
                 ORDER BY CASE f.status WHEN 'verified' THEN 0 ELSE 1 END, f.updated_at DESC",
            )?;
            let rows = stmt.query_map(params![repo], |row| Ok(row_to_fix(row)))?;
            rows.map(|r| r?).collect()
        })
        .await
    }

    async fn upsert_node(&self, node: &GraphNode) -> Result<(), StoreError> {
        let node = node.clone();
        let props = serde_json::to_string(&node.props_json)?;
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO graph_nodes (id, kind, label, props_json) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(id) DO UPDATE SET \
                   kind = excluded.kind, \
                   label = excluded.label, \
                   props_json = excluded.props_json",
                params![node.id, node.kind.as_str(), node.label, props],
            )?;
            Ok(())
        })
        .await
    }

    async fn upsert_edge(&self, edge: &GraphEdge) -> Result<(), StoreError> {
        let edge = edge.clone();
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO graph_edges \
                 (src_id, edge_type, dst_id, worked_count, failed_count, hit_count, last_seen_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(src_id, edge_type, dst_id) DO UPDATE SET \
                   worked_count = excluded.worked_count, \
                   failed_count = excluded.failed_count, \
                   hit_count = excluded.hit_count, \
                   last_seen_at = excluded.last_seen_at",
                params![
                    edge.src_id,
                    edge.edge_type.as_str(),
                    edge.dst_id,
                    u64_to_i64(edge.worked_count, "worked_count")?,
                    u64_to_i64(edge.failed_count, "failed_count")?,
                    u64_to_i64(edge.hit_count, "hit_count")?,
                    edge.last_seen_at,
                ],
            )?;
            Ok(())
        })
        .await
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
        let src_id = src_id.to_owned();
        let dst_id = dst_id.to_owned();
        let last_seen_at = last_seen_at.to_owned();
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO graph_edges \
                 (src_id, edge_type, dst_id, worked_count, failed_count, hit_count, last_seen_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(src_id, edge_type, dst_id) DO UPDATE SET \
                   worked_count = graph_edges.worked_count + excluded.worked_count, \
                   failed_count = graph_edges.failed_count + excluded.failed_count, \
                   hit_count = graph_edges.hit_count + excluded.hit_count, \
                   last_seen_at = excluded.last_seen_at",
                params![
                    src_id,
                    edge_type.as_str(),
                    dst_id,
                    u64_to_i64(worked_delta, "worked_delta")?,
                    u64_to_i64(failed_delta, "failed_delta")?,
                    u64_to_i64(hit_delta, "hit_delta")?,
                    last_seen_at,
                ],
            )?;
            conn.query_row(
                "SELECT src_id, edge_type, dst_id, worked_count, failed_count, hit_count, last_seen_at \
                 FROM graph_edges \
                 WHERE src_id = ?1 AND edge_type = ?2 AND dst_id = ?3",
                params![src_id, edge_type.as_str(), dst_id],
                |row| Ok(row_to_edge(row)),
            )?
        })
        .await
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
        let root = node_id.to_owned();
        let filter: Option<HashSet<EdgeType>> = edge_types.map(|s| s.iter().copied().collect());
        self.run(move |conn| {
            let mut results = Vec::new();
            let mut visited_nodes = HashSet::from([root.clone()]);
            let mut seen_edges = HashSet::new();
            let mut frontier = VecDeque::from([(root, 0_u8)]);

            let mut out_stmt = conn.prepare(
                "SELECT n.id, n.kind, n.label, n.props_json, \
                        e.src_id, e.edge_type, e.dst_id, e.worked_count, e.failed_count, e.hit_count, e.last_seen_at \
                 FROM graph_edges e \
                 JOIN graph_nodes n ON n.id = e.dst_id \
                 WHERE e.src_id = ?1",
            )?;
            let mut in_stmt = conn.prepare(
                "SELECT n.id, n.kind, n.label, n.props_json, \
                        e.src_id, e.edge_type, e.dst_id, e.worked_count, e.failed_count, e.hit_count, e.last_seen_at \
                 FROM graph_edges e \
                 JOIN graph_nodes n ON n.id = e.src_id \
                 WHERE e.dst_id = ?1",
            )?;

            while let Some((current, depth)) = frontier.pop_front() {
                if depth >= max_hops {
                    continue;
                }
                let next_hop = depth + 1;
                for (direction, stmt) in [
                    (EdgeDirection::Outgoing, &mut out_stmt),
                    (EdgeDirection::Incoming, &mut in_stmt),
                ] {
                    let rows = stmt.query_map(params![current], |row| {
                        let node = row_to_node_at(row, 0);
                        let edge = row_to_edge_at(row, 4);
                        Ok((node, edge))
                    })?;
                    for pair in rows {
                        let (node_res, edge_res) = pair?;
                        let node = node_res?;
                        let edge = edge_res?;
                        if filter.as_ref().is_some_and(|set| !set.contains(&edge.edge_type)) {
                            continue;
                        }
                        let edge_key = (edge.src_id.clone(), edge.edge_type, edge.dst_id.clone());
                        if !seen_edges.insert(edge_key) {
                            continue;
                        }
                        if visited_nodes.insert(node.id.clone()) && next_hop < max_hops {
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
        })
        .await
    }

    async fn fixes_for_pattern(&self, pattern_id: &str) -> Result<Vec<FixCandidate>, StoreError> {
        let src_id = format!("err:{pattern_id}");
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT f.id, f.summary, f.fix_type, f.pattern_before, f.pattern_after, \
                        f.guard_regex, f.status, f.scope, f.source, f.created_at, f.updated_at, \
                        e.src_id, e.edge_type, e.dst_id, e.worked_count, e.failed_count, e.hit_count, e.last_seen_at \
                 FROM graph_edges e \
                 JOIN fixes f ON ('fix:' || f.id) = e.dst_id \
                 WHERE e.src_id = ?1 AND e.edge_type = 'RESOLVED_BY' \
                 ORDER BY e.worked_count DESC, e.last_seen_at DESC",
            )?;
            let rows = stmt.query_map(params![src_id], |row| {
                let fix = row_to_fix(row);
                let edge = row_to_edge_at(row, 11);
                Ok((fix, edge))
            })?;
            let mut out = Vec::new();
            for pair in rows {
                let (fix_res, edge_res) = pair?;
                out.push(FixCandidate {
                    fix: fix_res?,
                    edge: edge_res?,
                });
            }
            Ok(out)
        })
        .await
    }

    async fn pattern_repo_count(&self, pattern_id: &str) -> Result<usize, StoreError> {
        let src_id = format!("err:{pattern_id}");
        self.run(move |conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(DISTINCT e.dst_id) \
                 FROM graph_edges e \
                 JOIN graph_nodes n ON n.id = e.dst_id \
                 WHERE e.src_id = ?1 AND e.edge_type = 'OCCURS_IN' AND n.kind = 'repo'",
                params![src_id],
                |row| row.get(0),
            )?;
            usize::try_from(count).map_err(|err| StoreError::InvalidData {
                context: "pattern_repo_count",
                detail: err.to_string(),
            })
        })
        .await
    }

    async fn pattern_contexts(&self, pattern_id: &str) -> Result<Vec<GraphNode>, StoreError> {
        let src_id = format!("err:{pattern_id}");
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT n.id, n.kind, n.label, n.props_json \
                 FROM graph_edges e \
                 JOIN graph_nodes n ON n.id = e.dst_id \
                 WHERE e.src_id = ?1 AND e.edge_type = 'OCCURS_IN'",
            )?;
            let rows = stmt.query_map(params![src_id], |row| Ok(row_to_node(row)))?;
            rows.map(|r| r?).collect()
        })
        .await
    }

    async fn is_fix_superseded_or_obsolete(
        &self,
        fix_id: &str,
        cli: Option<&str>,
        model: Option<&str>,
    ) -> Result<bool, StoreError> {
        let src_id = format!("fix:{fix_id}");
        let cli_node = cli.map(|c| format!("cli:{c}"));
        let model_node = model.map(|m| format!("model:{m}"));
        self.run(move |conn| {
            let flagged: bool = conn.query_row(
                "SELECT EXISTS ( \
                    SELECT 1 FROM graph_edges \
                    WHERE src_id = ?1 \
                      AND ( \
                          edge_type = 'SUPERSEDED_BY' \
                          OR (edge_type = 'OBSOLETE_IN' AND (dst_id = ?2 OR dst_id = ?3)) \
                      ) \
                 )",
                params![src_id, cli_node, model_node],
                |row| row.get(0),
            )?;
            Ok(flagged)
        })
        .await
    }

    async fn record_episode(&self, episode: &Episode) -> Result<(), StoreError> {
        let ep = episode.clone();
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO episodes \
                 (id, session_id, cli, model, os, shell, tool_name, phase, \
                  pattern_id, injected_fix_id, command_scrubbed, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    ep.id,
                    ep.session_id,
                    ep.cli.as_str(),
                    ep.model,
                    ep.os.as_str(),
                    ep.shell.as_str(),
                    ep.tool_name,
                    ep.phase.as_str(),
                    ep.pattern_id,
                    ep.injected_fix_id,
                    ep.command_scrubbed,
                    ep.created_at,
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn recent_session_episodes(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<Episode>, StoreError> {
        let session_id = session_id.to_owned();
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, cli, model, os, shell, tool_name, phase, \
                        pattern_id, injected_fix_id, command_scrubbed, created_at \
                 FROM episodes \
                 WHERE session_id = ?1 \
                 ORDER BY created_at DESC, rowid DESC \
                 LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(params![session_id, usize_to_i64(limit, "limit")?], |row| {
                    Ok(row_to_episode(row))
                })?;
            rows.map(|r| r?).collect()
        })
        .await
    }

    async fn prune_episodes_before(&self, cutoff_iso: &str) -> Result<usize, StoreError> {
        let cutoff = cutoff_iso.to_owned();
        self.run(move |conn| {
            let deleted = conn.execute(
                "DELETE FROM episodes WHERE created_at < ?1",
                params![cutoff],
            )?;
            Ok(deleted)
        })
        .await
    }

    async fn enqueue_outbox(
        &self,
        entity_type: &str,
        entity_id: &str,
        payload_json: &str,
        created_at: &str,
    ) -> Result<i64, StoreError> {
        let entity_type = entity_type.to_owned();
        let entity_id = entity_id.to_owned();
        let payload_json = payload_json.to_owned();
        let created_at = created_at.to_owned();
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO sync_outbox (entity_type, entity_id, payload_json, created_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![entity_type, entity_id, payload_json, created_at],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    async fn list_outbox(&self, limit: usize) -> Result<Vec<OutboxEntry>, StoreError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, entity_type, entity_id, payload_json, created_at \
                 FROM sync_outbox \
                 ORDER BY id ASC \
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![usize_to_i64(limit, "limit")?], |row| {
                Ok(OutboxEntry {
                    id: row.get(0)?,
                    entity_type: row.get(1)?,
                    entity_id: row.get(2)?,
                    payload_json: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?;
            rows.map(|r| r.map_err(StoreError::from)).collect()
        })
        .await
    }
}

fn row_to_node_at(row: &Row<'_>, offset: usize) -> Result<GraphNode, StoreError> {
    let kind_raw: String = row.get(offset + 1)?;
    let props_raw: String = row.get(offset + 3)?;
    Ok(GraphNode {
        id: row.get(offset)?,
        kind: parse_enum(&kind_raw, "graph_nodes.kind")?,
        label: row.get(offset + 2)?,
        props_json: serde_json::from_str(&props_raw)?,
    })
}

fn row_to_edge_at(row: &Row<'_>, offset: usize) -> Result<GraphEdge, StoreError> {
    let edge_type_raw: String = row.get(offset + 1)?;
    let worked_raw: i64 = row.get(offset + 3)?;
    let failed_raw: i64 = row.get(offset + 4)?;
    let hit_raw: i64 = row.get(offset + 5)?;
    Ok(GraphEdge {
        src_id: row.get(offset)?,
        edge_type: parse_enum(&edge_type_raw, "graph_edges.edge_type")?,
        dst_id: row.get(offset + 2)?,
        worked_count: i64_to_u64(worked_raw, "graph_edges.worked_count")?,
        failed_count: i64_to_u64(failed_raw, "graph_edges.failed_count")?,
        hit_count: i64_to_u64(hit_raw, "graph_edges.hit_count")?,
        last_seen_at: row.get(offset + 6)?,
    })
}
