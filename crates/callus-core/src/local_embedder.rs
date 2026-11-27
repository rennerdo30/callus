//! Built-in pure-Rust local CPU embedding engine (`BAAI/bge-small-en-v1.5`, 384 dims)
//! powered by Hugging Face `candle` (AVX2/NEON SIMD + Apple Accelerate on macOS).
//!
//! Zero `PyTorch`, zero CUDA, zero dynamic `.dll`/`.dylib`/`.so` dependencies.
//! Weights are lazy-downloaded atomically into `~/.callus/models/bge-small-en-v1.5/`
//! on first background backfill or search query, and never loaded on the `<5ms`
//! `callus hook` hot path.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use candle_core::{DType, Device, IndexOp as _, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use serde::{Deserialize, Serialize};
use sysinfo::{MemoryRefreshKind, RefreshKind, System};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};
use tokio::sync::OnceCell;

use crate::embedder::{DEFAULT_EMBEDDING_DIMS, DEFAULT_EMBEDDING_MODEL, EmbedError, Embedder};

/// Default Hugging Face repository hosting the MIT-licensed 384d `bge-small-en-v1.5` weights.
pub const DEFAULT_LOCAL_HF_REPO: &str = "BAAI/bge-small-en-v1.5";
/// Minimum logical CPU cores required before auto-enabling local embeddings.
pub const MIN_LOCAL_EMBED_CORES: usize = 2;
/// Minimum total host RAM (MiB) required before auto-enabling local embeddings (~4 GiB host).
pub const MIN_LOCAL_EMBED_RAM_MB: u64 = 3_500;
/// Minimum container cgroup RAM limit (MiB) required before auto-enabling local embeddings.
pub const MIN_LOCAL_CGROUP_RAM_MB: u64 = 1_024;

const MODEL_FILES: &[&str] = &["config.json", "tokenizer.json", "model.safetensors"];
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// Detected host hardware capabilities used to decide whether local CPU embeddings
/// should be enabled automatically by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalHardware {
    pub cpu_cores: usize,
    pub total_memory_mb: u64,
    pub cgroup_limit_mb: Option<u64>,
    pub supported: bool,
}

impl LocalHardware {
    /// Inspects logical CPU count, host RAM, and optional container cgroup memory limits.
    pub fn detect() -> Self {
        let cpu_cores = std::thread::available_parallelism().map_or(1, usize::from);
        let (total_memory_mb, cgroup_limit_mb) = match System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
        ) {
            Ok(sys) => {
                let ram_mb = sys.total_memory() / (1024 * 1024);
                let cgroup_mb = sys.cgroup_limits().and_then(|lim| {
                    let bytes = lim.total_memory;
                    if bytes == 0 || bytes == u64::MAX {
                        None
                    } else {
                        Some(bytes / (1024 * 1024))
                    }
                });
                (ram_mb, cgroup_mb)
            }
            Err(_) => (0, None),
        };
        Self::evaluate(cpu_cores, total_memory_mb, cgroup_limit_mb)
    }

    /// Pure evaluation helper (deterministic for unit testing).
    pub const fn evaluate(
        cpu_cores: usize,
        total_memory_mb: u64,
        cgroup_limit_mb: Option<u64>,
    ) -> Self {
        let cgroup_ok = match cgroup_limit_mb {
            Some(lim) => lim >= MIN_LOCAL_CGROUP_RAM_MB,
            None => true,
        };
        let supported = cpu_cores >= MIN_LOCAL_EMBED_CORES
            && total_memory_mb >= MIN_LOCAL_EMBED_RAM_MB
            && cgroup_ok;
        Self {
            cpu_cores,
            total_memory_mb,
            cgroup_limit_mb,
            supported,
        }
    }
}

/// Resolves the local model cache directory (`$CALLUS_MODEL_DIR` or `~/.callus/models/bge-small-en-v1.5`).
pub fn resolve_local_model_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CALLUS_MODEL_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    std::env::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".callus")
        .join("models")
        .join(DEFAULT_EMBEDDING_MODEL)
}

/// Returns true when all required local model files are already cached on disk.
pub fn is_local_model_cached(model_dir: &Path) -> bool {
    MODEL_FILES.iter().all(|name| {
        model_dir
            .join(name)
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() > 0)
    })
}

struct LoadedBert {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
    dims: usize,
}

impl fmt::Debug for LoadedBert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadedBert")
            .field("dims", &self.dims)
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

impl LoadedBert {
    fn load_from_dir(model_dir: &Path, expected_dims: usize) -> Result<Self, EmbedError> {
        let config_bytes = std::fs::read(model_dir.join("config.json")).map_err(|e| {
            EmbedError::Local(format!(
                "cannot read config.json in {}: {e}",
                model_dir.display()
            ))
        })?;
        let bert_cfg: BertConfig = serde_json::from_slice(&config_bytes)
            .map_err(|e| EmbedError::Local(format!("invalid BERT config.json: {e}")))?;
        let max_len = bert_cfg.max_position_embeddings.min(512);
        let actual_dims = bert_cfg.hidden_size;
        if actual_dims != expected_dims {
            return Err(EmbedError::DimensionMismatch {
                expected: expected_dims,
                actual: actual_dims,
            });
        }

        let mut tokenizer = Tokenizer::from_file(model_dir.join("tokenizer.json"))
            .map_err(|e| EmbedError::Local(format!("cannot load tokenizer.json: {e}")))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..PaddingParams::default()
        }));
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: max_len,
                ..TruncationParams::default()
            }))
            .map_err(|e| {
                EmbedError::Local(format!("cannot configure tokenizer truncation: {e}"))
            })?;

        let weights = std::fs::read(model_dir.join("model.safetensors")).map_err(|e| {
            EmbedError::Local(format!(
                "cannot read model.safetensors in {}: {e}",
                model_dir.display()
            ))
        })?;
        let device = Device::Cpu;
        let vb = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)
            .map_err(|e| EmbedError::Local(format!("cannot load safetensors weights: {e}")))?;
        let model = BertModel::load(vb, &bert_cfg)
            .map_err(|e| EmbedError::Local(format!("cannot build BERT model: {e}")))?;

        Ok(Self {
            model,
            tokenizer,
            device,
            dims: actual_dims,
        })
    }

    fn encode_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let inputs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let encodings = self
            .tokenizer
            .encode_batch(inputs, true)
            .map_err(|e| EmbedError::Local(format!("tokenization failed: {e}")))?;

        let batch_size = encodings.len();
        let seq_len = encodings.first().map_or(0, |e| e.get_ids().len());
        if seq_len == 0 {
            return Err(EmbedError::Local(
                "tokenizer produced empty sequence".to_owned(),
            ));
        }

        let mut token_ids = Vec::with_capacity(batch_size * seq_len);
        let mut token_types = Vec::with_capacity(batch_size * seq_len);
        let mut attention = Vec::with_capacity(batch_size * seq_len);
        for enc in &encodings {
            token_ids.extend_from_slice(enc.get_ids());
            token_types.extend_from_slice(enc.get_type_ids());
            attention.extend_from_slice(enc.get_attention_mask());
        }

        let ids_t = Tensor::from_vec(token_ids, (batch_size, seq_len), &self.device)
            .map_err(|e| EmbedError::Local(format!("input_ids tensor failed: {e}")))?;
        let types_t = Tensor::from_vec(token_types, (batch_size, seq_len), &self.device)
            .map_err(|e| EmbedError::Local(format!("token_type_ids tensor failed: {e}")))?;
        let mask_t = Tensor::from_vec(attention, (batch_size, seq_len), &self.device)
            .map_err(|e| EmbedError::Local(format!("attention_mask tensor failed: {e}")))?;

        let hidden = self
            .model
            .forward(&ids_t, &types_t, Some(&mask_t))
            .map_err(|e| EmbedError::Local(format!("BERT forward pass failed: {e}")))?;

        // Official BAAI/bge-small-en-v1.5 pooling: CLS token [:, 0, :] + L2 normalization.
        let cls = hidden
            .i((.., 0, ..))
            .map_err(|e| EmbedError::Local(format!("CLS slice failed: {e}")))?;
        let raw_vecs: Vec<Vec<f32>> = cls
            .to_vec2()
            .map_err(|e| EmbedError::Local(format!("tensor extraction failed: {e}")))?;

        let mut out = Vec::with_capacity(raw_vecs.len());
        for mut vec in raw_vecs {
            if vec.len() != self.dims {
                return Err(EmbedError::DimensionMismatch {
                    expected: self.dims,
                    actual: vec.len(),
                });
            }
            l2_normalize_in_place(&mut vec);
            out.push(vec);
        }
        Ok(out)
    }
}

fn l2_normalize_in_place(vec: &mut [f32]) {
    let norm = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 1e-12 {
        for v in vec.iter_mut() {
            *v /= norm;
        }
    }
}

/// Built-in local CPU embedder (`BAAI/bge-small-en-v1.5`, 384 dims) with lazy
/// atomic model download and non-blocking Tokio execution.
pub struct LocalEmbedder {
    model_dir: PathBuf,
    model_name: String,
    hf_repo: String,
    hf_endpoint: String,
    dims: usize,
    loaded: OnceCell<Arc<LoadedBert>>,
}

impl fmt::Debug for LocalEmbedder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalEmbedder")
            .field("model_dir", &self.model_dir)
            .field("model_name", &self.model_name)
            .field("dims", &self.dims)
            .field("initialized", &self.loaded.initialized())
            .finish_non_exhaustive()
    }
}

impl LocalEmbedder {
    /// Creates a lazy local embedder configured for `bge-small-en-v1.5` (384 dims).
    pub fn new(dims: usize, model_dir: Option<PathBuf>) -> Result<Self, EmbedError> {
        if dims != DEFAULT_EMBEDDING_DIMS {
            return Err(EmbedError::DimensionMismatch {
                expected: DEFAULT_EMBEDDING_DIMS,
                actual: dims,
            });
        }
        let hf_endpoint = std::env::var("CALLUS_HF_ENDPOINT")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "https://huggingface.co".to_owned());
        Ok(Self {
            model_dir: model_dir.unwrap_or_else(resolve_local_model_dir),
            model_name: DEFAULT_EMBEDDING_MODEL.to_owned(),
            hf_repo: DEFAULT_LOCAL_HF_REPO.to_owned(),
            hf_endpoint,
            dims,
            loaded: OnceCell::new(),
        })
    }

    /// Creates a local embedder targeting a custom cache directory and HF endpoint (for testing).
    pub fn with_endpoint(
        dims: usize,
        model_dir: PathBuf,
        hf_endpoint: &str,
    ) -> Result<Self, EmbedError> {
        if dims == 0 {
            return Err(EmbedError::InvalidConfig(
                "local embedding dims must be > 0".to_owned(),
            ));
        }
        Ok(Self {
            model_dir,
            model_name: DEFAULT_EMBEDDING_MODEL.to_owned(),
            hf_repo: DEFAULT_LOCAL_HF_REPO.to_owned(),
            hf_endpoint: hf_endpoint.trim_end_matches('/').to_owned(),
            dims,
            loaded: OnceCell::new(),
        })
    }

    /// Directory where the local model files are cached.
    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    async fn ensure_loaded(&self) -> Result<Arc<LoadedBert>, EmbedError> {
        let cell = self
            .loaded
            .get_or_try_init(|| async {
                self.ensure_model_files().await?;
                let dir = self.model_dir.clone();
                let dims = self.dims;
                let bert =
                    tokio::task::spawn_blocking(move || LoadedBert::load_from_dir(&dir, dims))
                        .await
                        .map_err(|e| {
                            EmbedError::Local(format!("model loader task failed: {e}"))
                        })??;
                Ok::<Arc<LoadedBert>, EmbedError>(Arc::new(bert))
            })
            .await?;
        Ok(Arc::clone(cell))
    }

    async fn ensure_model_files(&self) -> Result<(), EmbedError> {
        if is_local_model_cached(&self.model_dir) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.model_dir).map_err(|e| {
            EmbedError::Local(format!(
                "cannot create model cache dir {}: {e}",
                self.model_dir.display()
            ))
        })?;

        let client = reqwest::Client::builder()
            .timeout(DOWNLOAD_TIMEOUT)
            .user_agent(concat!("callus/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| EmbedError::Http(format!("cannot build download HTTP client: {e}")))?;

        for filename in MODEL_FILES {
            let dest = self.model_dir.join(filename);
            if dest.metadata().is_ok_and(|m| m.is_file() && m.len() > 0) {
                continue;
            }
            let url = format!(
                "{}/{}/resolve/main/{filename}",
                self.hf_endpoint, self.hf_repo
            );
            let resp = client.get(&url).send().await.map_err(|e| {
                if e.is_timeout() {
                    EmbedError::Timeout(DOWNLOAD_TIMEOUT)
                } else {
                    EmbedError::Http(format!("failed to download {url}: {e}"))
                }
            })?;
            if !resp.status().is_success() {
                return Err(EmbedError::Http(format!(
                    "model download {url} returned HTTP {}",
                    resp.status()
                )));
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| EmbedError::Http(format!("failed reading body for {url}: {e}")))?;
            if bytes.is_empty() {
                return Err(EmbedError::InvalidResponse(format!(
                    "downloaded empty model file {filename}"
                )));
            }
            let tmp = self.model_dir.join(format!("{filename}.part"));
            std::fs::write(&tmp, &bytes)
                .map_err(|e| EmbedError::Local(format!("cannot write {}: {e}", tmp.display())))?;
            std::fs::rename(&tmp, &dest).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                EmbedError::Local(format!("cannot finalize {}: {e}", dest.display()))
            })?;
        }
        Ok(())
    }
}

#[async_trait]
impl Embedder for LocalEmbedder {
    fn dims(&self) -> usize {
        self.dims
    }

    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "local"
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }

    async fn embed(&self, text: &str) -> Result<Option<Vec<f32>>, EmbedError> {
        match self.embed_batch_strict(&[text.to_owned()]).await {
            Ok(mut vecs) => Ok(vecs.pop()),
            Err(err @ (EmbedError::DimensionMismatch { .. } | EmbedError::InvalidConfig(_))) => {
                Err(err)
            }
            // Degrade cleanly to BM25 + graph if offline before first model download.
            Err(
                EmbedError::Timeout(_)
                | EmbedError::Http(_)
                | EmbedError::InvalidResponse(_)
                | EmbedError::Local(_),
            ) => Ok(None),
        }
    }

    async fn embed_batch_strict(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let bert = self.ensure_loaded().await?;
        let owned = texts.to_vec();
        tokio::task::spawn_blocking(move || bert.encode_batch(&owned))
            .await
            .map_err(|e| EmbedError::Local(format!("embedding worker panicked: {e}")))?
    }
}
