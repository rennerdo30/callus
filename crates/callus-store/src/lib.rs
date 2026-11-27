//! Unified storage trait, SQLite WAL hybrid backend, and self-learning engine.

pub mod engine;
pub mod error;
pub mod ranking;
pub mod sqlite;
pub mod traits;

pub use engine::{EngineConfig, IngestOutcome, LearningEngine};
pub use error::StoreError;
pub use ranking::{
    DEFAULT_HALF_LIFE_DAYS, DEFAULT_MIN_INJECT_CONFIDENCE, RRF_K, WILSON_Z, context_multiplier,
    effective_confidence, hybrid_relevance, parse_iso_epoch_secs, recency_decay,
    wilson_lower_bound,
};
pub use sqlite::{DEFAULT_EMBEDDING_DIMS, SqliteStore};
pub use traits::{
    Bm25Hit, EdgeDirection, FixCandidate, KnowledgeStore, Neighbor, OutboxEntry, VectorHit,
};
