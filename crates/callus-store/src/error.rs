//! Storage-layer errors shared by all `KnowledgeStore` implementations.

use callus_core::CallusError;

/// Errors returned by storage backends.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("PostgreSQL error: {0}")]
    Postgres(#[from] sqlx::Error),
    #[error("embedding dimension mismatch: store expects {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },
    #[error("invalid stored data ({context}): {detail}")]
    InvalidData {
        context: &'static str,
        detail: String,
    },
    #[error("entity not found: {0}")]
    NotFound(String),
    #[error("background storage task failed: {0}")]
    TaskJoin(String),
    #[error(transparent)]
    Core(#[from] CallusError),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}
