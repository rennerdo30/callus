//! Shared error type.

/// Errors returned by Callus crates.
#[derive(Debug, thiserror::Error)]
pub enum CallusError {
    /// A string did not name a variant of a domain enum.
    #[error("unknown {kind} value: {value:?}")]
    UnknownVariant { kind: &'static str, value: String },
    /// A redaction pattern (usually a user deny-list entry) did not compile.
    #[error("invalid redaction pattern: {0}")]
    Pattern(#[from] regex::Error),
    #[error("{context}: {source}")]
    Io {
        context: String,
        source: std::io::Error,
    },
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// A command that is documented but not built yet.
    #[error("`callus {0}` is not implemented yet (see docs/ROADMAP.md)")]
    NotImplemented(&'static str),
}
