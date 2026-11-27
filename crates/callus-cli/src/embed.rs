//! `callus embed`: inspect hardware detection, active embedding backend, local model
//! cache status, and optionally backfill pending patterns on demand.

use std::{path::PathBuf, sync::Arc, time::Instant};

use callus_core::{
    CallusError, EmbeddingConfig, EmbeddingProvider, LocalHardware, build_embedder,
    is_local_model_cached, resolve_local_model_dir,
};
use callus_store::{KnowledgeStore, SqliteStore};

use crate::{
    adapters::now_iso_utc,
    hook::{configured_dims, resolve_db_path},
    worker::consolidate_once,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Download the local model (if using local provider) and backfill all pending embeddings now
    #[arg(long)]
    backfill: bool,
    /// Override local SQLite database path
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
}

fn print_status(
    cfg: &EmbeddingConfig,
    embedder: &Arc<dyn callus_core::Embedder>,
    hw: &LocalHardware,
) {
    let model_dir = resolve_local_model_dir();
    let cached = is_local_model_cached(&model_dir);

    println!("Provider:  {}", embedder.provider_name());
    println!(
        "Model:     {} ({}d)",
        embedder.model_name(),
        embedder.dims()
    );
    if let Some(url) = &cfg.url {
        println!("Endpoint:  {url}");
    }
    println!(
        "Hardware:  {} CPU cores, {} MiB RAM, cgroup limit: {} (auto-local: {})",
        hw.cpu_cores,
        hw.total_memory_mb,
        hw.cgroup_limit_mb
            .map_or_else(|| "none".to_owned(), |m| format!("{m} MiB")),
        if hw.supported { "yes" } else { "no" }
    );
    if cfg.provider == EmbeddingProvider::Local {
        println!(
            "Cache:     {} ({})",
            model_dir.display(),
            if cached {
                "cached"
            } else {
                "downloads lazily on first backfill/query"
            }
        );
    }
}

pub(crate) fn run(args: Args) -> Result<(), CallusError> {
    let hw = LocalHardware::detect();
    let cfg = EmbeddingConfig::from_env().map_err(|e| CallusError::Io {
        context: "invalid embedding configuration".to_owned(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()),
    })?;
    let dims = if cfg.provider == EmbeddingProvider::None {
        configured_dims()
    } else {
        cfg.dims
    };
    let embedder = build_embedder(&cfg).map_err(|e| CallusError::Io {
        context: "cannot initialize embedder".to_owned(),
        source: std::io::Error::other(e.to_string()),
    })?;
    print_status(&cfg, &embedder, &hw);

    let db_path = resolve_db_path(args.db);

    if !db_path.exists() {
        println!("Database:  {} (not created yet)", db_path.display());
        if !args.backfill {
            return Ok(());
        }
    }

    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| CallusError::Io {
            context: format!("cannot create {}", parent.display()),
            source,
        })?;
    }
    let store: Arc<dyn KnowledgeStore> = Arc::new(SqliteStore::open(&db_path, dims).map_err(
        |e| CallusError::Io {
            context: "cannot open sqlite store".to_owned(),
            source: std::io::Error::other(e.to_string()),
        },
    )?);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|source| CallusError::Io {
            context: "cannot start tokio runtime".to_owned(),
            source,
        })?;

    rt.block_on(async {
        let pending_before = store
            .list_pending_embeddings(1000)
            .await
            .map_or(0, |v| v.len());
        println!("Pending:   {pending_before} pattern(s)");

        if !args.backfill {
            return Ok::<(), CallusError>(());
        }
        if !embedder.is_enabled() {
            println!(
                "Embedding provider is disabled (set EMBEDDING_PROVIDER=local or EMBEDDING_URL)."
            );
            return Ok(());
        }

        let started = Instant::now();
        // Verify / warm up the embedder strictly so any download/config error is reported clearly.
        embedder.health_check().await.map_err(|e| CallusError::Io {
            context: "embedder health check failed".to_owned(),
            source: std::io::Error::other(e.to_string()),
        })?;

        let mut total_embedded = 0_usize;
        loop {
            let report = consolidate_once(&store, &embedder, 90, &now_iso_utc()).await;
            total_embedded += report.backfilled_embeddings;
            if report.backfilled_embeddings == 0 {
                break;
            }
        }
        let elapsed_ms = started.elapsed().as_millis();
        println!("Backfilled {total_embedded} pattern(s) in {elapsed_ms} ms.");
        Ok(())
    })
}
