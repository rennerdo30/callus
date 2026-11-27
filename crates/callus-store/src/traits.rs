//! Backend-agnostic `KnowledgeStore` trait and query result types.
//!
//! Implemented by `SqliteStore` locally and by `PgStore` on the central server.

use async_trait::async_trait;
use callus_core::{EdgeType, Episode, ErrorPattern, Fix, FixStatus, GraphEdge, GraphNode, Scope};
use serde::{Deserialize, Serialize};

use crate::StoreError;

/// One BM25 / full-text search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bm25Hit {
    pub pattern_id: String,
    /// Higher is more relevant (negated SQLite FTS5 `bm25()`).
    pub score: f64,
    /// 1-based rank in the BM25 result set.
    pub rank: usize,
}

/// One cosine-distance vector similarity hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorHit {
    pub pattern_id: String,
    /// Cosine distance (lower is closer).
    pub distance: f64,
    /// 1-based rank in the vector result set.
    pub rank: usize,
}

/// Direction of a traversed graph edge relative to the query node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeDirection {
    Outgoing,
    Incoming,
}

/// One node reached during a 1-2 hop graph neighborhood traversal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Neighbor {
    pub node: GraphNode,
    pub edge: GraphEdge,
    pub direction: EdgeDirection,
    pub hop: u8,
}

/// A candidate fix together with its `RESOLVED_BY` edge metrics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixCandidate {
    pub fix: Fix,
    pub edge: GraphEdge,
}

/// Pending local mutation waiting to sync to a central server (v0.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub id: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub payload_json: String,
    pub created_at: String,
}

/// Unified asynchronous storage interface for Callus (SQLite local, PostgreSQL server).
#[async_trait]
pub trait KnowledgeStore: Send + Sync {
    /// Configured vector embedding dimensionality for this store.
    fn embedding_dims(&self) -> usize;

    // --- Error Patterns ---

    /// Inserts a new pattern or updates an existing one matching `signature_hash`.
    async fn upsert_pattern(&self, pattern: &ErrorPattern) -> Result<ErrorPattern, StoreError>;

    async fn get_pattern(&self, id: &str) -> Result<Option<ErrorPattern>, StoreError>;

    async fn find_pattern_by_signature(
        &self,
        signature_hash: &str,
    ) -> Result<Option<ErrorPattern>, StoreError>;

    async fn list_patterns(
        &self,
        scope: Option<Scope>,
        repo_hash: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ErrorPattern>, StoreError>;

    /// Updates a pattern's visibility scope and optional repository binding.
    async fn update_pattern_scope(
        &self,
        id: &str,
        scope: Scope,
        repo_hash: Option<&str>,
        updated_at: &str,
    ) -> Result<(), StoreError>;

    async fn list_pending_embeddings(&self, limit: usize) -> Result<Vec<ErrorPattern>, StoreError>;

    // --- Search & Embeddings ---

    async fn upsert_pattern_embedding(
        &self,
        pattern_id: &str,
        embedding: &[f32],
    ) -> Result<(), StoreError>;

    async fn search_bm25(
        &self,
        query: &str,
        tool_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Bm25Hit>, StoreError>;

    async fn search_vector(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorHit>, StoreError>;

    // --- Fixes ---

    async fn upsert_fix(&self, fix: &Fix) -> Result<Fix, StoreError>;

    async fn get_fix(&self, id: &str) -> Result<Option<Fix>, StoreError>;

    async fn update_fix_status(
        &self,
        id: &str,
        status: FixStatus,
        updated_at: &str,
    ) -> Result<(), StoreError>;

    /// Lists active `pre_tool_guard` fixes eligible for `repo_hash` (strict repo isolation).
    async fn list_active_guards(&self, repo_hash: Option<&str>) -> Result<Vec<Fix>, StoreError>;

    // --- Knowledge Graph ---

    async fn upsert_node(&self, node: &GraphNode) -> Result<(), StoreError>;

    async fn upsert_edge(&self, edge: &GraphEdge) -> Result<(), StoreError>;

    /// Atomically adds counter deltas to an edge, creating it if absent.
    #[expect(
        clippy::too_many_arguments,
        reason = "explicit atomic edge counter deltas"
    )]
    async fn record_edge_feedback(
        &self,
        src_id: &str,
        edge_type: EdgeType,
        dst_id: &str,
        worked_delta: u64,
        failed_delta: u64,
        hit_delta: u64,
        last_seen_at: &str,
    ) -> Result<GraphEdge, StoreError>;

    /// Traverses 1 to 2 hops around `node_id`, optionally filtered by `edge_types`.
    async fn neighbors(
        &self,
        node_id: &str,
        edge_types: Option<&[EdgeType]>,
        max_hops: u8,
    ) -> Result<Vec<Neighbor>, StoreError>;

    /// Returns all fixes linked from `err:<pattern_id>` via `RESOLVED_BY`.
    async fn fixes_for_pattern(&self, pattern_id: &str) -> Result<Vec<FixCandidate>, StoreError>;

    /// Counts distinct `repo:*` nodes linked from `err:<pattern_id>` via `OCCURS_IN`.
    async fn pattern_repo_count(&self, pattern_id: &str) -> Result<usize, StoreError>;

    /// Returns context nodes (`environment`, `cli`, `model`, `repo`, `tool`) linked via `OCCURS_IN`.
    async fn pattern_contexts(&self, pattern_id: &str) -> Result<Vec<GraphNode>, StoreError>;

    /// True if `fix:<fix_id>` has an outgoing `SUPERSEDED_BY` edge or an `OBSOLETE_IN`
    /// edge targeting the caller's CLI or model.
    async fn is_fix_superseded_or_obsolete(
        &self,
        fix_id: &str,
        cli: Option<&str>,
        model: Option<&str>,
    ) -> Result<bool, StoreError>;

    // --- Local Episodes ---

    async fn record_episode(&self, episode: &Episode) -> Result<(), StoreError>;

    /// Most recent episodes for `session_id`, newest first.
    async fn recent_session_episodes(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<Episode>, StoreError>;

    /// Deletes episodes older than `cutoff_iso` and returns the number removed.
    async fn prune_episodes_before(&self, cutoff_iso: &str) -> Result<usize, StoreError>;

    // --- Sync Outbox (v0.2 ready) ---

    async fn enqueue_outbox(
        &self,
        entity_type: &str,
        entity_id: &str,
        payload_json: &str,
        created_at: &str,
    ) -> Result<i64, StoreError>;

    async fn list_outbox(&self, limit: usize) -> Result<Vec<OutboxEntry>, StoreError>;
}
