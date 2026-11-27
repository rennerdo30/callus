//! Unified storage trait and local SQLite (WAL + FTS5 + sqlite-vec + graph) backend.

pub mod error;
pub mod sqlite;
pub mod traits;

pub use error::StoreError;
pub use sqlite::{DEFAULT_EMBEDDING_DIMS, SqliteStore};
pub use traits::{
    Bm25Hit, EdgeDirection, FixCandidate, KnowledgeStore, Neighbor, OutboxEntry, VectorHit,
};
