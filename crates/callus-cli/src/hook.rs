//! `callus hook`: ultra-fast, fail-open local hook runner.
//!
//! Never calls remote embedders on the hot path and never blocks the calling CLI
//! on internal errors.

use std::{
    io::{Read as _, Write as _},
    path::PathBuf,
};

use callus_core::{DEFAULT_EMBEDDING_DIMS, Scrubber, ScrubberConfig};
use callus_store::{EngineConfig, KnowledgeStore, LearningEngine, SqliteStore};
use serde_json::Value;

use crate::adapters::{
    AdapterCli, HookEventArg, format_hook_output, iso_utc_days_ago, normalize_hook_payload,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Calling CLI adapter
    #[arg(long, value_enum)]
    cli: AdapterCli,
    /// Hook lifecycle event
    #[arg(long, value_enum)]
    event: HookEventArg,
    /// Override local SQLite database path (default: `$CALLUS_DB_PATH` or `~/.callus/callus.db`)
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
}

/// Runs one hook invocation, always failing open (exit 0) so agents are never stalled.
pub(crate) fn run(args: Args) {
    if let Err(err) = run_inner(args)
        && std::env::var_os("CALLUS_DEBUG").is_some()
    {
        eprintln!("callus hook warning (fail-open): {err}");
    }
}

pub(crate) fn resolve_db_path(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }
    if let Some(env_path) = std::env::var_os("CALLUS_DB_PATH")
        && !env_path.is_empty()
    {
        return PathBuf::from(env_path);
    }
    std::env::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".callus")
        .join("callus.db")
}

pub(crate) fn configured_dims() -> usize {
    std::env::var("EMBEDDING_DIMS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&d| d > 0)
        .unwrap_or(DEFAULT_EMBEDDING_DIMS)
}

fn run_inner(args: Args) -> Result<(), String> {
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .map_err(|e| format!("stdin read failed: {e}"))?;

    let payload: Value = if raw.trim().is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(&raw).map_err(|e| format!("invalid JSON stdin: {e}"))?
    };

    let db_path = resolve_db_path(args.db);
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create db dir: {e}"))?;
    }

    let store = SqliteStore::open(&db_path, configured_dims())
        .map_err(|e| format!("cannot open sqlite store: {e}"))?;

    // Single-threaded runtime avoids multi-thread pool spawn overhead on <5ms hooks.
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|e| format!("cannot start tokio runtime: {e}"))?;

    let Some(event) = normalize_hook_payload(args.cli, args.event, &payload) else {
        // Stop event: prune local short-retention episodes older than 7 days.
        let cutoff = iso_utc_days_ago(7);
        let _ = rt.block_on(store.prune_episodes_before(&cutoff));
        return Ok(());
    };

    let repo_root = payload
        .get("cwd")
        .or_else(|| payload.get("workspacePath"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let scrubber = Scrubber::new(&ScrubberConfig {
        repo_root,
        home_dir: std::env::home_dir().map(|p| p.to_string_lossy().into_owned()),
        extra_hostnames: Vec::new(),
        deny_regexes: Vec::new(),
    })
    .map_err(|e| format!("scrubber init failed: {e}"))?;

    let engine = LearningEngine::new(store, EngineConfig::default());
    let outcome = rt
        .block_on(engine.ingest_event(&event, &scrubber, None))
        .map_err(|e| format!("ingest failed: {e}"))?;

    if let Some(output) = format_hook_output(args.cli, &outcome) {
        let mut line =
            serde_json::to_string(&output).map_err(|e| format!("serialize failed: {e}"))?;
        line.push('\n');
        std::io::stdout()
            .write_all(line.as_bytes())
            .map_err(|e| format!("stdout write failed: {e}"))?;
    }
    Ok(())
}
