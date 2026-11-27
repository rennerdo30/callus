//! Core types of Callus: the domain model, deterministic error signatures and
//! the mandatory client-side privacy scrubber.

pub mod domain;
pub mod embedder;
pub mod error;
pub mod local_embedder;
pub mod scrubber;
pub mod signature;

pub use domain::*;
pub use embedder::{
    DEFAULT_EMBEDDING_DIMS, DEFAULT_EMBEDDING_MODEL, DEFAULT_EMBEDDING_TIMEOUT_MS, EmbedError,
    Embedder, EmbeddingConfig, EmbeddingProvider, NoopEmbedder, OpenAiCompatibleEmbedder,
    build_embedder,
};
pub use error::CallusError;
pub use local_embedder::{
    DEFAULT_LOCAL_HF_REPO, LocalEmbedder, LocalHardware, is_local_model_cached,
    resolve_local_model_dir,
};
pub use scrubber::{RedactionCategory, RedactionHit, ScrubResult, Scrubber, ScrubberConfig};
pub use signature::{compute_signature_hash, normalize_error_template};
