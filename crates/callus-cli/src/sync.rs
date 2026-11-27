//! `callus sync`: bidirectional scoped sync between local SQLite and a central
//! Callus server (`POST /api/v1/sync/push`, `GET /api/v1/sync/pull`).
//!
//! `Scope::Personal` records and raw `episodes` NEVER leave the local machine.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use callus_core::{
    CallusError, EdgeType, ErrorPattern, Fix, GraphEdge, GraphNode, Scope, Scrubber, ScrubberConfig,
};
use callus_store::{KnowledgeStore, SqliteStore};
use serde::{Deserialize, Serialize};

use crate::{
    adapters::{compute_repo_hash, now_iso_utc},
    hook::{configured_dims, resolve_db_path},
};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Central Callus server base URL (env `CALLUS_SERVER_URL`)
    #[arg(long, value_name = "URL")]
    server: Option<String>,
    /// Bearer API token (env `CALLUS_API_TOKEN`)
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,
    /// Explicit repository hash filter (defaults to current git repo)
    #[arg(long, value_name = "HASH")]
    repo_hash: Option<String>,
    /// Override local SQLite database path
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
    /// Only push local scoped deltas to the server
    #[arg(long)]
    push_only: bool,
    /// Only pull remote scoped updates from the server
    #[arg(long)]
    pull_only: bool,
    /// Preview outbound counts without contacting the server
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SyncPatternDelta {
    pub pattern: ErrorPattern,
    pub occurrence_delta: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SyncEdgeDelta {
    pub src_id: String,
    pub edge_type: EdgeType,
    pub dst_id: String,
    pub worked_delta: u64,
    pub failed_delta: u64,
    pub hit_delta: u64,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SyncPushRequest {
    pub client_id: String,
    pub patterns: Vec<SyncPatternDelta>,
    pub fixes: Vec<Fix>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<SyncEdgeDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SyncPushResponse {
    pub accepted_patterns: usize,
    pub accepted_fixes: usize,
    pub accepted_edges: usize,
    pub rejected_personal: usize,
    pub server_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SyncPullResponse {
    pub cursor: String,
    pub patterns: Vec<ErrorPattern>,
    pub fixes: Vec<Fix>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncStateFile {
    pull_cursor: String,
    /// Last pushed `occurrence_count` per pattern `signature_hash` for exact CRDT deltas.
    pushed_pattern_counts: HashMap<String, u64>,
    /// Last pushed `(worked, failed, hit)` per edge key `src|type|dst`.
    pushed_edge_counts: HashMap<String, (u64, u64, u64)>,
}

impl SyncStateFile {
    fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) -> Result<(), CallusError> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json).map_err(|source| CallusError::Io {
            context: format!("cannot save sync state {}", path.display()),
            source,
        })
    }
}

fn state_path_for(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push(".sync-state.json");
    PathBuf::from(os)
}

fn edge_key(e: &GraphEdge) -> String {
    format!("{}|{}|{}", e.src_id, e.edge_type.as_str(), e.dst_id)
}

pub(crate) fn run(args: Args) -> Result<(), CallusError> {
    let server_url = args
        .server
        .or_else(|| {
            std::env::var("CALLUS_SERVER_URL")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .ok_or_else(|| CallusError::Io {
            context: "sync requires --server <URL> or CALLUS_SERVER_URL".to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing server URL"),
        })?;
    let server_url = server_url.trim_end_matches('/').to_owned();
    let token = args.token.or_else(|| {
        std::env::var("CALLUS_API_TOKEN")
            .ok()
            .filter(|v| !v.trim().is_empty())
    });

    let repo_hash = args.repo_hash.or_else(|| {
        std::env::current_dir()
            .ok()
            .as_deref()
            .and_then(compute_repo_hash)
    });
    let db_path = resolve_db_path(args.db);
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| CallusError::Io {
            context: format!("cannot create {}", parent.display()),
            source,
        })?;
    }
    let store = SqliteStore::open(&db_path, configured_dims()).map_err(|e| CallusError::Io {
        context: "cannot open local sqlite store".to_owned(),
        source: std::io::Error::other(e.to_string()),
    })?;
    let scrubber = Scrubber::new(&ScrubberConfig {
        home_dir: std::env::home_dir().map(|p| p.to_string_lossy().into_owned()),
        ..ScrubberConfig::default()
    })?;
    let state_path = state_path_for(&db_path);
    let mut state = SyncStateFile::load(&state_path);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|source| CallusError::Io {
            context: "cannot start tokio runtime".to_owned(),
            source,
        })?;

    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        if !args.pull_only {
            push_to_server(
                &client,
                &server_url,
                token.as_deref(),
                &store,
                &scrubber,
                &mut state,
                repo_hash.as_deref(),
                args.dry_run,
            )
            .await?;
        }
        if !args.push_only && !args.dry_run {
            pull_from_server(
                &client,
                &server_url,
                token.as_deref(),
                &store,
                &mut state,
                repo_hash.as_deref(),
            )
            .await?;
        }
        state
            .save(&state_path)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok::<(), std::io::Error>(())
    })
    .map_err(|source| CallusError::Io {
        context: "sync failed".to_owned(),
        source,
    })
}

#[expect(clippy::too_many_arguments, reason = "sync step context")]
async fn push_to_server(
    client: &reqwest::Client,
    server_url: &str,
    token: Option<&str>,
    store: &SqliteStore,
    scrubber: &Scrubber,
    state: &mut SyncStateFile,
    repo_hash: Option<&str>,
    dry_run: bool,
) -> Result<(), std::io::Error> {
    let push_req = build_push_payload(store, scrubber, state, repo_hash).await?;
    if dry_run {
        println!(
            "[dry-run] would push {} patterns, {} fixes, {} edges to {server_url}",
            push_req.patterns.len(),
            push_req.fixes.len(),
            push_req.edges.len()
        );
        return Ok(());
    }
    if push_req.patterns.is_empty() && push_req.fixes.is_empty() && push_req.edges.is_empty() {
        return Ok(());
    }
    let mut req = client
        .post(format!("{server_url}/api/v1/sync/push"))
        .json(&push_req);
    if let Some(tok) = token {
        req = req.bearer_auth(tok);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            let ack: SyncPushResponse = resp
                .json()
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            for p in &push_req.patterns {
                *state
                    .pushed_pattern_counts
                    .entry(p.pattern.signature_hash.clone())
                    .or_insert(0) += p.occurrence_delta;
            }
            for e in &push_req.edges {
                let key = format!("{}|{}|{}", e.src_id, e.edge_type.as_str(), e.dst_id);
                let entry = state.pushed_edge_counts.entry(key).or_insert((0, 0, 0));
                entry.0 += e.worked_delta;
                entry.1 += e.failed_delta;
                entry.2 += e.hit_delta;
            }
            let outbox = store
                .list_outbox(500)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let ids: Vec<i64> = outbox.into_iter().map(|o| o.id).collect();
            let _ = store.delete_outbox_entries(&ids).await;
            println!(
                "pushed {} patterns, {} fixes, {} edges (rejected personal: {})",
                ack.accepted_patterns,
                ack.accepted_fixes,
                ack.accepted_edges,
                ack.rejected_personal
            );
            Ok(())
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            Err(std::io::Error::other(format!(
                "sync push failed ({status}): {body}"
            )))
        }
        Err(err) => {
            let payload_json = serde_json::to_string(&push_req).unwrap_or_else(|_| "{}".to_owned());
            let _ = store
                .enqueue_outbox(
                    "sync_batch",
                    &push_req.client_id,
                    &payload_json,
                    &now_iso_utc(),
                )
                .await;
            Err(std::io::Error::other(format!(
                "server unreachable (queued to sync_outbox): {err}"
            )))
        }
    }
}

async fn pull_from_server(
    client: &reqwest::Client,
    server_url: &str,
    token: Option<&str>,
    store: &SqliteStore,
    state: &mut SyncStateFile,
    repo_hash: Option<&str>,
) -> Result<(), std::io::Error> {
    let mut url = format!(
        "{server_url}/api/v1/sync/pull?since={}&limit=200",
        state.pull_cursor
    );
    if let Some(repo) = repo_hash {
        url.push_str("&repo_hash=");
        url.push_str(repo);
    }
    let mut req = client.get(&url);
    if let Some(tok) = token {
        req = req.bearer_auth(tok);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| std::io::Error::other(format!("sync pull failed: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(std::io::Error::other(format!(
            "sync pull failed ({status}): {body}"
        )));
    }
    let pulled: SyncPullResponse = resp
        .json()
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    apply_pulled_snapshot(store, &pulled, state).await?;
    println!(
        "pulled {} patterns, {} fixes, {} edges (cursor={})",
        pulled.patterns.len(),
        pulled.fixes.len(),
        pulled.edges.len(),
        state.pull_cursor
    );
    Ok(())
}

async fn build_push_payload(
    store: &SqliteStore,
    scrubber: &Scrubber,
    state: &SyncStateFile,
    repo_hash: Option<&str>,
) -> Result<SyncPushRequest, std::io::Error> {
    // STRICT privacy boundary: NEVER select Scope::Personal or raw episodes.
    let sync_scopes = [Scope::Team, Scope::Repo, Scope::PublicPack];
    let all_patterns = store
        .list_patterns_updated_since("", &sync_scopes, repo_hash, 500)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let mut pattern_deltas = Vec::new();
    let mut allowed_pattern_ids = HashSet::new();
    for mut pat in all_patterns {
        if pat.scope == Scope::Personal {
            continue;
        }
        allowed_pattern_ids.insert(pat.id.clone());
        let prev = state
            .pushed_pattern_counts
            .get(&pat.signature_hash)
            .copied()
            .unwrap_or(0);
        let delta = pat.occurrence_count.saturating_sub(prev);
        if delta > 0 {
            pat.title = scrubber.scrub_text(&pat.title).scrubbed;
            pat.error_template = scrubber.scrub_text(&pat.error_template).scrubbed;
            pattern_deltas.push(SyncPatternDelta {
                pattern: pat,
                occurrence_delta: delta,
            });
        }
    }

    let fixes_raw = store
        .list_fixes_updated_since("", &sync_scopes, 500)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut fixes = Vec::new();
    let mut allowed_fix_ids = HashSet::new();
    for mut fix in fixes_raw {
        if fix.scope == Scope::Personal {
            continue;
        }
        allowed_fix_ids.insert(fix.id.clone());
        fix.summary = scrubber.scrub_text(&fix.summary).scrubbed;
        fix.pattern_before = fix.pattern_before.map(|s| scrubber.scrub_text(&s).scrubbed);
        fix.pattern_after = fix.pattern_after.map(|s| scrubber.scrub_text(&s).scrubbed);
        fixes.push(fix);
    }

    let raw_edges = store
        .list_edges_updated_since("", 1000)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut edge_deltas = Vec::new();
    let mut node_ids = HashSet::new();
    for e in raw_edges {
        // Only export edges anchored to non-personal synced patterns or fixes.
        let src_ok = e
            .src_id
            .strip_prefix("err:")
            .is_some_and(|id| allowed_pattern_ids.contains(id))
            || e.src_id
                .strip_prefix("fix:")
                .is_some_and(|id| allowed_fix_ids.contains(id));
        if !src_ok {
            continue;
        }
        let key = edge_key(&e);
        let (pw, pf, ph) = state
            .pushed_edge_counts
            .get(&key)
            .copied()
            .unwrap_or((0, 0, 0));
        let dw = e.worked_count.saturating_sub(pw);
        let df = e.failed_count.saturating_sub(pf);
        let dh = e.hit_count.saturating_sub(ph);
        if dw > 0 || df > 0 || dh > 0 {
            node_ids.insert(e.src_id.clone());
            node_ids.insert(e.dst_id.clone());
            edge_deltas.push(SyncEdgeDelta {
                src_id: e.src_id,
                edge_type: e.edge_type,
                dst_id: e.dst_id,
                worked_delta: dw,
                failed_delta: df,
                hit_delta: dh,
                last_seen_at: e.last_seen_at,
            });
        }
    }

    let id_vec: Vec<String> = node_ids.into_iter().collect();
    let nodes = store
        .list_nodes_by_ids(&id_vec)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    Ok(SyncPushRequest {
        client_id: format!("cli-{}", std::process::id()),
        patterns: pattern_deltas,
        fixes,
        nodes,
        edges: edge_deltas,
    })
}

async fn apply_pulled_snapshot(
    store: &SqliteStore,
    pulled: &SyncPullResponse,
    state: &mut SyncStateFile,
) -> Result<(), std::io::Error> {
    for pat in &pulled.patterns {
        if pat.scope == Scope::Personal {
            continue;
        }
        let existing = store
            .find_pattern_by_signature(&pat.signature_hash)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let current_local = existing.as_ref().map_or(0, |p| p.occurrence_count);
        let target = current_local.max(pat.occurrence_count);
        let delta = target.saturating_sub(current_local);
        let stored = store
            .merge_pattern_delta(pat, delta)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        state
            .pushed_pattern_counts
            .insert(stored.signature_hash, stored.occurrence_count);
    }

    for fix in &pulled.fixes {
        if fix.scope == Scope::Personal {
            continue;
        }
        store
            .merge_fix_lww(fix)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }

    for node in &pulled.nodes {
        store
            .upsert_node(node)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }

    for edge in &pulled.edges {
        store
            .upsert_edge(edge)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        state.pushed_edge_counts.insert(
            edge_key(edge),
            (edge.worked_count, edge.failed_count, edge.hit_count),
        );
    }

    if !pulled.cursor.is_empty() && pulled.cursor > state.pull_cursor {
        state.pull_cursor.clone_from(&pulled.cursor);
    }
    Ok(())
}
