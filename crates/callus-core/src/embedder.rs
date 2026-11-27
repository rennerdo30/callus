//! External OpenAI-compatible (`llama.cpp`) embedding client with strict timeouts
//! and graceful offline fallback.
//!
//! Callus never bundles a local GPU runtime; embeddings are requested over HTTP
//! from an external `/v1/embeddings` server configured via environment variables.

use std::{fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Default embedding vector dimensionality (`bge-small-en-v1.5`).
pub const DEFAULT_EMBEDDING_DIMS: usize = 384;
/// Default model identifier sent in OpenAI-compatible embedding requests.
pub const DEFAULT_EMBEDDING_MODEL: &str = "bge-small-en-v1.5";
/// Default strict HTTP timeout so queries never stall on an unreachable embedder.
pub const DEFAULT_EMBEDDING_TIMEOUT_MS: u64 = 1500;

/// Configured embedding backend mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmbeddingProvider {
    OpenAiCompatible,
    None,
}

/// Environment-driven embedding configuration.
#[derive(Clone, PartialEq, Eq)]
pub struct EmbeddingConfig {
    pub provider: EmbeddingProvider,
    pub url: Option<String>,
    pub model: String,
    pub dims: usize,
    pub api_key: Option<String>,
    pub timeout_ms: u64,
}

impl fmt::Debug for EmbeddingConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddingConfig")
            .field("provider", &self.provider)
            .field("url", &self.url)
            .field("model", &self.model)
            .field("dims", &self.dims)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: EmbeddingProvider::None,
            url: None,
            model: DEFAULT_EMBEDDING_MODEL.to_owned(),
            dims: DEFAULT_EMBEDDING_DIMS,
            api_key: None,
            timeout_ms: DEFAULT_EMBEDDING_TIMEOUT_MS,
        }
    }
}

impl EmbeddingConfig {
    /// Reads embedding settings from standard `EMBEDDING_*` environment variables.
    pub fn from_env() -> Result<Self, EmbedError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Reads embedding settings from an arbitrary key lookup (testable without mutating env).
    pub fn from_lookup<F>(lookup: F) -> Result<Self, EmbedError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let non_empty = |key: &str| {
            lookup(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };

        let url = non_empty("EMBEDDING_URL");
        let provider = match non_empty("EMBEDDING_PROVIDER").as_deref() {
            Some("none") => EmbeddingProvider::None,
            Some("openai-compatible") => EmbeddingProvider::OpenAiCompatible,
            Some(other) => {
                return Err(EmbedError::InvalidConfig(format!(
                    "unsupported EMBEDDING_PROVIDER {other:?}; expected \"openai-compatible\" or \"none\""
                )));
            }
            None => {
                if url.is_some() {
                    EmbeddingProvider::OpenAiCompatible
                } else {
                    EmbeddingProvider::None
                }
            }
        };

        if provider == EmbeddingProvider::OpenAiCompatible && url.is_none() {
            return Err(EmbedError::InvalidConfig(
                "EMBEDDING_PROVIDER=openai-compatible requires EMBEDDING_URL".to_owned(),
            ));
        }

        let dims = match non_empty("EMBEDDING_DIMS") {
            Some(raw) => raw.parse::<usize>().map_err(|err| {
                EmbedError::InvalidConfig(format!("invalid EMBEDDING_DIMS {raw:?}: {err}"))
            })?,
            None => DEFAULT_EMBEDDING_DIMS,
        };
        if dims == 0 {
            return Err(EmbedError::InvalidConfig(
                "EMBEDDING_DIMS must be greater than 0".to_owned(),
            ));
        }

        let timeout_ms = match non_empty("EMBEDDING_TIMEOUT_MS") {
            Some(raw) => raw.parse::<u64>().map_err(|err| {
                EmbedError::InvalidConfig(format!("invalid EMBEDDING_TIMEOUT_MS {raw:?}: {err}"))
            })?,
            None => DEFAULT_EMBEDDING_TIMEOUT_MS,
        };
        if timeout_ms == 0 {
            return Err(EmbedError::InvalidConfig(
                "EMBEDDING_TIMEOUT_MS must be greater than 0".to_owned(),
            ));
        }

        Ok(Self {
            provider,
            url,
            model: non_empty("EMBEDDING_MODEL")
                .unwrap_or_else(|| DEFAULT_EMBEDDING_MODEL.to_owned()),
            dims,
            api_key: non_empty("EMBEDDING_API_KEY"),
            timeout_ms,
        })
    }
}

/// Errors produced by embedding configuration or remote `/v1/embeddings` calls.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("invalid embedding configuration: {0}")]
    InvalidConfig(String),
    #[error("embedding dimension mismatch: configured {expected}, server returned {actual}")]
    DimensionMismatch { expected: usize, actual: usize },
    #[error("embedding request timed out after {0:?}")]
    Timeout(Duration),
    #[error("embedding HTTP request failed: {0}")]
    Http(String),
    #[error("invalid embedding response: {0}")]
    InvalidResponse(String),
}

/// Embedding provider interface used by search and background backfill.
#[async_trait]
pub trait Embedder: Send + Sync + fmt::Debug {
    /// Expected vector length.
    fn dims(&self) -> usize;

    /// True when an external embedding endpoint is configured.
    fn is_enabled(&self) -> bool;

    /// Embeds `text`, returning `Ok(None)` when disabled or transiently unreachable.
    /// Dimension mismatches always return `Err(EmbedError::DimensionMismatch)`.
    async fn embed(&self, text: &str) -> Result<Option<Vec<f32>>, EmbedError>;

    /// Embeds a batch of inputs with strict error reporting (for backfill / health checks).
    async fn embed_batch_strict(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;

    /// Verifies connectivity and dimensionality against the configured provider.
    async fn health_check(&self) -> Result<Option<usize>, EmbedError> {
        if !self.is_enabled() {
            return Ok(None);
        }
        let vectors = self
            .embed_batch_strict(&["callus health check".to_owned()])
            .await?;
        let actual = vectors.first().map_or(0, Vec::len);
        Ok(Some(actual))
    }
}

/// Disabled embedder used when `EMBEDDING_URL` is unset or `EMBEDDING_PROVIDER=none`.
#[derive(Debug, Clone)]
pub struct NoopEmbedder {
    dims: usize,
}

impl NoopEmbedder {
    pub const fn new(dims: usize) -> Self {
        Self { dims }
    }
}

#[async_trait]
impl Embedder for NoopEmbedder {
    fn dims(&self) -> usize {
        self.dims
    }

    fn is_enabled(&self) -> bool {
        false
    }

    async fn embed(&self, _text: &str) -> Result<Option<Vec<f32>>, EmbedError> {
        Ok(None)
    }

    async fn embed_batch_strict(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(Vec::new())
    }
}

/// HTTP client targeting an external OpenAI-compatible `/v1/embeddings` server (`llama.cpp`).
pub struct OpenAiCompatibleEmbedder {
    client: reqwest::Client,
    url: String,
    model: String,
    dims: usize,
    api_key: Option<String>,
    timeout: Duration,
}

impl fmt::Debug for OpenAiCompatibleEmbedder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiCompatibleEmbedder")
            .field("url", &self.url)
            .field("model", &self.model)
            .field("dims", &self.dims)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    index: usize,
    embedding: Vec<f32>,
}

impl OpenAiCompatibleEmbedder {
    pub fn new(config: &EmbeddingConfig) -> Result<Self, EmbedError> {
        let Some(url) = config.url.clone().filter(|u| !u.is_empty()) else {
            return Err(EmbedError::InvalidConfig(
                "OpenAiCompatibleEmbedder requires a non-empty url".to_owned(),
            ));
        };
        if config.dims == 0 {
            return Err(EmbedError::InvalidConfig(
                "embedding dims must be greater than 0".to_owned(),
            ));
        }
        let timeout = Duration::from_millis(config.timeout_ms.max(1));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| EmbedError::InvalidConfig(format!("cannot build HTTP client: {err}")))?;
        Ok(Self {
            client,
            url,
            model: config.model.clone(),
            dims: config.dims,
            api_key: config.api_key.clone(),
            timeout,
        })
    }

    async fn request_embeddings(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut req = self.client.post(&self.url).json(&EmbeddingRequest {
            model: &self.model,
            input: texts,
        });
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let response = req.send().await.map_err(|err| {
            if err.is_timeout() {
                EmbedError::Timeout(self.timeout)
            } else {
                EmbedError::Http(err.to_string())
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(EmbedError::Http(format!("status {status}: {body}")));
        }

        let mut parsed: EmbeddingResponse = response
            .json()
            .await
            .map_err(|err| EmbedError::InvalidResponse(err.to_string()))?;

        if parsed.data.len() != texts.len() {
            return Err(EmbedError::InvalidResponse(format!(
                "expected {} embeddings, server returned {}",
                texts.len(),
                parsed.data.len()
            )));
        }

        parsed.data.sort_by_key(|d| d.index);
        let mut vectors = Vec::with_capacity(parsed.data.len());
        for datum in parsed.data {
            if datum.embedding.len() != self.dims {
                return Err(EmbedError::DimensionMismatch {
                    expected: self.dims,
                    actual: datum.embedding.len(),
                });
            }
            vectors.push(datum.embedding);
        }
        Ok(vectors)
    }
}

#[async_trait]
impl Embedder for OpenAiCompatibleEmbedder {
    fn dims(&self) -> usize {
        self.dims
    }

    fn is_enabled(&self) -> bool {
        true
    }

    async fn embed(&self, text: &str) -> Result<Option<Vec<f32>>, EmbedError> {
        match self.request_embeddings(&[text.to_owned()]).await {
            Ok(mut vecs) => Ok(vecs.pop()),
            // Never silently ignore misconfigured vector dimensions or invalid config.
            Err(err @ (EmbedError::DimensionMismatch { .. } | EmbedError::InvalidConfig(_))) => {
                Err(err)
            }
            // Degrade cleanly to BM25 + signature hash + graph on offline/timeout failures.
            Err(EmbedError::Timeout(_) | EmbedError::Http(_) | EmbedError::InvalidResponse(_)) => {
                Ok(None)
            }
        }
    }

    async fn embed_batch_strict(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.request_embeddings(texts).await
    }
}

/// Builds the appropriate [`Embedder`] from [`EmbeddingConfig`].
pub fn build_embedder(config: &EmbeddingConfig) -> Result<Arc<dyn Embedder>, EmbedError> {
    match config.provider {
        EmbeddingProvider::None => Ok(Arc::new(NoopEmbedder::new(config.dims))),
        EmbeddingProvider::OpenAiCompatible => Ok(Arc::new(OpenAiCompatibleEmbedder::new(config)?)),
    }
}
