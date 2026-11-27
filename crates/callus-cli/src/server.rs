//! `callus serve`: `LiteVM` central HTTP server (`axum`), REST API, Bearer auth,
//! and Streamable HTTP MCP (`/mcp` via `rmcp`).

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use callus_core::{
    CallusError, EdgeType, Embedder, EmbeddingConfig, EmbeddingProvider, FixStatus, NodeKind,
    Scope, Scrubber, ScrubberConfig, SearchQuery, build_embedder,
};
use callus_store::{EngineConfig, KnowledgeStore, LearningEngine, PgStore, SqliteStore};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    adapters::now_iso_utc,
    export::{
        ExportFormat, ExportScope, collect_candidates, render_hooks_json, render_markdown,
        render_pack_json,
    },
    hook::{configured_dims, resolve_db_path},
    mcp::CallusMcpServer,
    sync::{SyncPullResponse, SyncPushRequest, SyncPushResponse},
    ui,
    worker::{consolidate_once, spawn_worker},
};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Listen address (`host:port`, env `CALLUS_BIND`)
    #[arg(long, default_value = "0.0.0.0:8080")]
    bind: String,
    /// PostgreSQL connection URL for central team server (env `DATABASE_URL`)
    #[arg(long, value_name = "URL")]
    database_url: Option<String>,
    /// Optional local SQLite DB path when running `callus serve` without PostgreSQL
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
    /// Optional Bearer API token protecting `/api/v1/*` and `/mcp` (env `CALLUS_API_TOKEN`)
    #[arg(long, value_name = "TOKEN")]
    api_token: Option<String>,
    /// Max PostgreSQL pool connections (`LiteVM` default: 12)
    #[arg(long, default_value_t = 12)]
    max_connections: u32,
    /// Background consolidation interval in seconds (`0` disables automatic loop)
    #[arg(long, default_value_t = 30)]
    worker_interval_secs: u64,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub engine: Arc<LearningEngine<Arc<dyn KnowledgeStore>>>,
    pub embedder: Arc<dyn Embedder>,
    pub scrubber: Arc<Scrubber>,
    pub backend: &'static str,
    pub api_token: Option<Arc<str>>,
}

#[derive(Debug, Deserialize)]
struct ListPatternsParams {
    scope: Option<String>,
    repo_hash: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct GraphParams {
    hops: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct UpdateScopeBody {
    scope: Scope,
    repo_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateFixStatusBody {
    status: FixStatus,
}

#[derive(Debug, Deserialize)]
struct ScrubBody {
    text: String,
}

#[derive(Debug, Deserialize)]
struct ExportQueryParams {
    format: Option<String>,
    scope: Option<String>,
    repo_hash: Option<String>,
    budget_lines: Option<usize>,
    stale_days: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SyncPullParams {
    since: Option<String>,
    repo_hash: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

fn err_resp(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ApiError { error: msg.into() })).into_response()
}

/// Constant-time token comparison to avoid timing side channels.
fn token_matches(expected: &str, provided: &str) -> bool {
    let a = expected.as_bytes();
    let b = provided.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0_u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

async fn bearer_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let Some(expected) = state.api_token.as_deref() else {
        return next.run(request).await;
    };
    let provided = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| {
            raw.strip_prefix("Bearer ")
                .or_else(|| raw.strip_prefix("bearer "))
        });

    match provided {
        Some(token) if token_matches(expected, token) => next.run(request).await,
        _ => err_resp(StatusCode::UNAUTHORIZED, "missing or invalid Bearer token"),
    }
}

pub(crate) fn build_router(state: AppState) -> Router {
    let mcp_server = CallusMcpServer::new(
        Arc::clone(&state.engine),
        Arc::clone(&state.embedder),
        Arc::clone(&state.scrubber),
    );
    let mcp_service = StreamableHttpService::new(
        move || Ok(mcp_server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let protected = Router::new()
        .route("/api/v1/overview", get(overview))
        .route("/api/v1/patterns", get(list_patterns))
        .route("/api/v1/patterns/{id}", get(get_pattern_detail))
        .route("/api/v1/patterns/{id}/scope", post(update_pattern_scope))
        .route("/api/v1/fixes/{id}/status", post(update_fix_status))
        .route("/api/v1/graph/{node_id}", get(get_graph))
        .route("/api/v1/search", post(search_fixes))
        .route("/api/v1/export", get(export_rules))
        .route("/api/v1/scrub-test", post(scrub_test))
        .route("/api/v1/sync/push", post(sync_push))
        .route("/api/v1/sync/pull", get(sync_pull))
        .route("/api/v1/admin/consolidate", post(run_consolidation))
        .nest_service("/mcp", mcp_service)
        .route_layer(middleware::from_fn_with_state(state.clone(), bearer_auth));

    Router::new()
        .route("/healthz", get(healthz))
        .merge(protected)
        .fallback(ui::serve_embedded)
        .with_state(state)
}

async fn healthz(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "backend": state.backend,
        "embedding_enabled": state.embedder.is_enabled(),
        "embedding_provider": state.embedder.provider_name(),
        "embedding_model": state.embedder.model_name(),
        "embedding_dims": state.engine.store().embedding_dims(),
        "auth_enabled": state.api_token.is_some(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Debug, Serialize)]
struct ContextStat {
    kind: NodeKind,
    label: String,
    hits: u64,
}

async fn summarize_fix_counts(
    store: &Arc<dyn KnowledgeStore>,
    patterns: &[callus_core::ErrorPattern],
) -> (usize, usize, usize) {
    let mut verified = 0_usize;
    let mut candidate = 0_usize;
    let mut obsolete = 0_usize;
    let mut seen = HashSet::new();
    for pat in patterns {
        if let Ok(cands) = store.fixes_for_pattern(&pat.id).await {
            for cand in cands {
                if !seen.insert(cand.fix.id.clone()) {
                    continue;
                }
                match cand.fix.status {
                    FixStatus::Verified => verified += 1,
                    FixStatus::Candidate => candidate += 1,
                    FixStatus::Deprecated | FixStatus::Obsolete => obsolete += 1,
                }
            }
        }
    }
    (verified, candidate, obsolete)
}

async fn overview(State(state): State<AppState>) -> Response {
    let store = state.engine.store();
    let patterns = match store.list_patterns(None, None, 200).await {
        Ok(p) => p,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let pending = match store.list_pending_embeddings(1000).await {
        Ok(p) => p.len(),
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let guards = match store.list_active_guards(None).await {
        Ok(g) => g.len(),
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let edges = match store.list_edges_updated_since("", 500).await {
        Ok(e) => e,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let mut retries_saved = 0_u64;
    let mut context_hits: HashMap<String, u64> = HashMap::new();
    for edge in &edges {
        if edge.edge_type == EdgeType::ResolvedBy {
            retries_saved = retries_saved.saturating_add(edge.worked_count);
        } else if edge.edge_type == EdgeType::OccursIn && edge.hit_count > 0 {
            *context_hits.entry(edge.dst_id.clone()).or_insert(0) += edge.hit_count;
        }
    }

    let ctx_ids: Vec<String> = context_hits.keys().cloned().collect();
    let mut context_breakdown: Vec<ContextStat> = store
        .list_nodes_by_ids(&ctx_ids)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|n| {
            matches!(
                n.kind,
                NodeKind::Environment | NodeKind::Cli | NodeKind::Model | NodeKind::Repo
            )
        })
        .map(|n| ContextStat {
            hits: context_hits.get(&n.id).copied().unwrap_or(0),
            kind: n.kind,
            label: n.label,
        })
        .collect();
    context_breakdown.sort_by(|a, b| b.hits.cmp(&a.hits).then_with(|| a.label.cmp(&b.label)));
    context_breakdown.truncate(16);

    let (verified_fix_count, candidate_fix_count, obsolete_fix_count) =
        summarize_fix_counts(store, &patterns).await;

    let (mut personal, mut repo, mut team, mut public_pack) = (0_usize, 0_usize, 0_usize, 0_usize);
    for pat in &patterns {
        match pat.scope {
            Scope::Personal => personal += 1,
            Scope::Repo => repo += 1,
            Scope::Team => team += 1,
            Scope::PublicPack => public_pack += 1,
        }
    }

    let total_occurrences: u64 = patterns.iter().map(|p| p.occurrence_count).sum();
    // Only show real failures that have actually occurred in Hotspots (never unhit 0x imported packs).
    let hotspots: Vec<_> = patterns
        .iter()
        .filter(|p| p.occurrence_count > 0)
        .take(10)
        .cloned()
        .collect();

    Json(json!({
        "pattern_count": patterns.len(),
        "total_occurrences": total_occurrences,
        "retries_saved": retries_saved,
        "active_guard_count": guards,
        "verified_fix_count": verified_fix_count,
        "candidate_fix_count": candidate_fix_count,
        "obsolete_fix_count": obsolete_fix_count,
        "pending_embeddings": pending,
        "embedding_enabled": state.embedder.is_enabled(),
        "embedding_provider": state.embedder.provider_name(),
        "embedding_model": state.embedder.model_name(),
        "embedding_dims": store.embedding_dims(),
        "backend": state.backend,
        "version": env!("CARGO_PKG_VERSION"),
        "scope_counts": {
            "personal": personal,
            "repo": repo,
            "team": team,
            "public_pack": public_pack,
        },
        "context_breakdown": context_breakdown,
        "hotspots": hotspots,
    }))
    .into_response()
}

async fn list_patterns(
    State(state): State<AppState>,
    Query(params): Query<ListPatternsParams>,
) -> Response {
    let scope = match params.scope.as_deref() {
        Some(raw) if !raw.is_empty() => match raw.parse::<Scope>() {
            Ok(s) => Some(s),
            Err(e) => return err_resp(StatusCode::BAD_REQUEST, e.to_string()),
        },
        _ => None,
    };
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    match state
        .engine
        .store()
        .list_patterns(scope, params.repo_hash.as_deref(), limit)
        .await
    {
        Ok(patterns) => Json(json!({ "patterns": patterns })).into_response(),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn get_pattern_detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let pattern = match state.engine.store().get_pattern(&id).await {
        Ok(Some(p)) => p,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, format!("pattern {id} not found")),
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let fixes = match state.engine.store().fixes_for_pattern(&id).await {
        Ok(f) => f,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let contexts = match state.engine.store().pattern_contexts(&id).await {
        Ok(c) => c,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    Json(json!({
        "pattern": pattern,
        "fixes": fixes,
        "contexts": contexts,
    }))
    .into_response()
}

async fn update_pattern_scope(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateScopeBody>,
) -> Response {
    match state
        .engine
        .store()
        .update_pattern_scope(&id, body.scope, body.repo_hash.as_deref(), &now_iso_utc())
        .await
    {
        Ok(()) => Json(json!({ "ok": true, "id": id, "scope": body.scope })).into_response(),
        Err(callus_store::StoreError::NotFound(m)) => err_resp(StatusCode::NOT_FOUND, m),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn update_fix_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateFixStatusBody>,
) -> Response {
    match state
        .engine
        .store()
        .update_fix_status(&id, body.status, &now_iso_utc())
        .await
    {
        Ok(()) => Json(json!({ "ok": true, "id": id, "status": body.status })).into_response(),
        Err(callus_store::StoreError::NotFound(m)) => err_resp(StatusCode::NOT_FOUND, m),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn get_graph(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Query(params): Query<GraphParams>,
) -> Response {
    let hops = params.hops.unwrap_or(2).clamp(1, 2);
    match state.engine.store().neighbors(&node_id, None, hops).await {
        Ok(neighbors) => Json(json!({ "root": node_id, "neighbors": neighbors })).into_response(),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn search_fixes(State(state): State<AppState>, Json(query): Json<SearchQuery>) -> Response {
    let scrubbed = state.scrubber.scrub_text(&query.text).scrubbed;
    let query_vec = match state.embedder.embed(&scrubbed).await {
        Ok(v) => v,
        Err(e) => return err_resp(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    let vector_used = query_vec.is_some();
    let clean_query = SearchQuery {
        text: scrubbed,
        limit: query.limit.clamp(1, 50),
        ..query
    };
    match state
        .engine
        .search_fixes(
            &clean_query,
            query_vec.as_deref(),
            None,
            &now_iso_utc(),
            &HashSet::new(),
        )
        .await
    {
        Ok(hits) => Json(json!({ "hits": hits, "vector_used": vector_used })).into_response(),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn export_rules(
    State(state): State<AppState>,
    Query(params): Query<ExportQueryParams>,
) -> Response {
    let format = match params.format.as_deref().unwrap_or("agents-md") {
        "agents-md" => ExportFormat::AgentsMd,
        "claude-md" => ExportFormat::ClaudeMd,
        "gemini-md" => ExportFormat::GeminiMd,
        "hooks" => ExportFormat::Hooks,
        "pack" => ExportFormat::Pack,
        other => return err_resp(StatusCode::BAD_REQUEST, format!("unknown format {other:?}")),
    };
    let scope = match params.scope.as_deref().unwrap_or("all") {
        "repo" => ExportScope::Repo,
        "global" => ExportScope::Global,
        "all" => ExportScope::All,
        other => return err_resp(StatusCode::BAD_REQUEST, format!("unknown scope {other:?}")),
    };
    let now_iso = now_iso_utc();
    let candidates = match collect_candidates(
        state.engine.store(),
        scope,
        params.repo_hash.as_deref(),
        params.stale_days.unwrap_or(90),
        &now_iso,
    )
    .await
    {
        Ok(c) => c,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let budget = params.budget_lines.unwrap_or(40).clamp(1, 200);
    let content = match format {
        ExportFormat::AgentsMd => render_markdown("AGENTS.md", scope, &candidates, budget, None),
        ExportFormat::ClaudeMd => render_markdown("CLAUDE.md", scope, &candidates, budget, None),
        ExportFormat::GeminiMd => render_markdown("GEMINI.md", scope, &candidates, budget, None),
        ExportFormat::Hooks => {
            let guards = match state
                .engine
                .store()
                .list_active_guards(params.repo_hash.as_deref())
                .await
            {
                Ok(g) => g,
                Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            };
            match render_hooks_json(&guards) {
                Ok(s) => s,
                Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            }
        }
        ExportFormat::Pack => match render_pack_json(&candidates, &now_iso) {
            Ok(s) => s,
            Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        },
    };
    Json(json!({ "content": content, "candidate_count": candidates.len() })).into_response()
}

async fn scrub_test(State(state): State<AppState>, Json(body): Json<ScrubBody>) -> Response {
    Json(state.scrubber.scrub_text(&body.text)).into_response()
}

async fn sync_push(State(state): State<AppState>, Json(req): Json<SyncPushRequest>) -> Response {
    let mut accepted_patterns = 0_usize;
    let mut accepted_fixes = 0_usize;
    let mut accepted_edges = 0_usize;
    let mut rejected_personal = 0_usize;

    for mut item in req.patterns {
        if item.pattern.scope == Scope::Personal {
            rejected_personal += 1;
            continue;
        }
        item.pattern.title = state.scrubber.scrub_text(&item.pattern.title).scrubbed;
        item.pattern.error_template = state
            .scrubber
            .scrub_text(&item.pattern.error_template)
            .scrubbed;
        if let Err(e) = state
            .engine
            .store()
            .merge_pattern_delta(&item.pattern, item.occurrence_delta)
            .await
        {
            return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        accepted_patterns += 1;
    }

    for mut fix in req.fixes {
        if fix.scope == Scope::Personal {
            rejected_personal += 1;
            continue;
        }
        fix.summary = state.scrubber.scrub_text(&fix.summary).scrubbed;
        fix.pattern_before = fix
            .pattern_before
            .map(|s| state.scrubber.scrub_text(&s).scrubbed);
        fix.pattern_after = fix
            .pattern_after
            .map(|s| state.scrubber.scrub_text(&s).scrubbed);
        if let Err(e) = state.engine.store().merge_fix_lww(&fix).await {
            return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        accepted_fixes += 1;
    }

    for node in req.nodes {
        if let Err(e) = state.engine.store().upsert_node(&node).await {
            return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }

    for edge in req.edges {
        if let Err(e) = state
            .engine
            .store()
            .record_edge_feedback(
                &edge.src_id,
                edge.edge_type,
                &edge.dst_id,
                edge.worked_delta,
                edge.failed_delta,
                edge.hit_delta,
                &edge.last_seen_at,
            )
            .await
        {
            return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        accepted_edges += 1;
    }

    Json(SyncPushResponse {
        accepted_patterns,
        accepted_fixes,
        accepted_edges,
        rejected_personal,
        server_time: now_iso_utc(),
    })
    .into_response()
}

async fn sync_pull(
    State(state): State<AppState>,
    Query(params): Query<SyncPullParams>,
) -> Response {
    let since = params.since.as_deref().unwrap_or("");
    let limit = params.limit.unwrap_or(200).clamp(1, 500);
    let scopes = [Scope::Team, Scope::Repo, Scope::PublicPack];

    let patterns = match state
        .engine
        .store()
        .list_patterns_updated_since(since, &scopes, params.repo_hash.as_deref(), limit)
        .await
    {
        Ok(p) => p,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let fixes = match state
        .engine
        .store()
        .list_fixes_updated_since(since, &scopes, limit)
        .await
    {
        Ok(f) => f,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let all_edges = match state
        .engine
        .store()
        .list_edges_updated_since(since, limit.saturating_mul(2))
        .await
    {
        Ok(e) => e,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let pat_ids: HashSet<&str> = patterns.iter().map(|p| p.id.as_str()).collect();
    let fix_ids: HashSet<&str> = fixes.iter().map(|f| f.id.as_str()).collect();
    let mut node_ids = HashSet::new();
    let mut edges = Vec::new();
    for edge in all_edges {
        let relevant = edge
            .src_id
            .strip_prefix("err:")
            .is_some_and(|id| pat_ids.contains(id))
            || edge
                .src_id
                .strip_prefix("fix:")
                .is_some_and(|id| fix_ids.contains(id));
        if relevant {
            node_ids.insert(edge.src_id.clone());
            node_ids.insert(edge.dst_id.clone());
            edges.push(edge);
        }
    }
    let id_vec: Vec<String> = node_ids.into_iter().collect();
    let nodes = match state.engine.store().list_nodes_by_ids(&id_vec).await {
        Ok(n) => n,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let cursor = patterns
        .iter()
        .map(|p| p.updated_at.as_str())
        .chain(fixes.iter().map(|f| f.updated_at.as_str()))
        .chain(edges.iter().map(|e| e.last_seen_at.as_str()))
        .max()
        .unwrap_or(since)
        .to_owned();

    Json(SyncPullResponse {
        cursor,
        patterns,
        fixes,
        nodes,
        edges,
    })
    .into_response()
}

async fn run_consolidation(State(state): State<AppState>) -> Response {
    let report = consolidate_once(state.engine.store(), &state.embedder, 90, &now_iso_utc()).await;
    Json(report).into_response()
}

pub(crate) fn run_local_ui(port: u16, db: Option<PathBuf>) -> Result<(), CallusError> {
    run(Args {
        bind: format!("127.0.0.1:{port}"),
        database_url: None,
        db,
        api_token: None,
        max_connections: 4,
        worker_interval_secs: 60,
    })
}

pub(crate) fn run(args: Args) -> Result<(), CallusError> {
    let bind_raw = std::env::var("CALLUS_BIND")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(args.bind);
    let addr: SocketAddr = bind_raw.parse().map_err(|e| CallusError::Io {
        context: format!("invalid bind address {bind_raw:?}"),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, e),
    })?;

    let db_url = args.database_url.or_else(|| {
        std::env::var("DATABASE_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
    });
    let api_token = args
        .api_token
        .or_else(|| {
            std::env::var("CALLUS_API_TOKEN")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .map(Arc::<str>::from);

    let embed_cfg = EmbeddingConfig::from_env().unwrap_or_default();
    let dims = if embed_cfg.provider == EmbeddingProvider::None {
        configured_dims()
    } else {
        embed_cfg.dims
    };
    let embedder = build_embedder(&embed_cfg).map_err(|e| CallusError::Io {
        context: "embedder configuration failed".to_owned(),
        source: std::io::Error::other(e.to_string()),
    })?;
    let scrubber = Arc::new(Scrubber::new(&ScrubberConfig::default())?);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|source| CallusError::Io {
            context: "cannot start tokio runtime".to_owned(),
            source,
        })?;

    rt.block_on(async move {
        let (store, backend): (Arc<dyn KnowledgeStore>, &'static str) = if let Some(url) = db_url {
            let pg = PgStore::connect(&url, dims, args.max_connections)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            (Arc::new(pg), "postgres")
        } else {
            let db_path = resolve_db_path(args.db);
            if let Some(parent) = db_path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            let sq = SqliteStore::open(&db_path, dims)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            (Arc::new(sq), "sqlite")
        };

        let interval_secs = std::env::var("CALLUS_WORKER_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(args.worker_interval_secs);
        spawn_worker(Arc::clone(&store), Arc::clone(&embedder), interval_secs);

        let state = AppState {
            engine: Arc::new(LearningEngine::new(store, EngineConfig::default())),
            embedder,
            scrubber,
            backend,
            api_token,
        };
        let app = build_router(state);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        println!("callus serve listening on http://{local_addr} (backend={backend})");
        axum::serve(listener, app).await?;
        Ok::<(), std::io::Error>(())
    })
    .map_err(|source| CallusError::Io {
        context: "server exited with error".to_owned(),
        source,
    })
}
