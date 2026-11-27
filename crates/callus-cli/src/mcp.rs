//! `callus mcp`: official Rust MCP server (`rmcp`) over stdio exposing
//! `callus_search`, `callus_report`, `callus_feedback`, and `callus_export_candidates`.

#![allow(clippy::unused_async_trait_impl)]

use std::{collections::HashSet, path::PathBuf, sync::Arc};

use callus_core::{
    CallusError, CliKind, ContextInfo, EdgeType, EmbeddingConfig, EmbeddingStatus, ErrorPattern,
    Fix, FixSource, FixStatus, FixType, GraphNode, NodeKind, OsKind, Scope, Scrubber,
    ScrubberConfig, SearchQuery, ShellKind, build_embedder, compute_signature_hash,
    normalize_error_template,
};
use callus_store::{EngineConfig, KnowledgeStore, LearningEngine, SqliteStore};
use rmcp::{
    ServiceExt as _,
    handler::server::wrapper::{Json, Parameters},
    schemars, tool, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    adapters::now_iso_utc,
    export::{ExportCandidate, ExportScope, collect_candidates},
    hook::{configured_dims, resolve_db_path},
};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Override local SQLite database path (default: `$CALLUS_DB_PATH` or `~/.callus/callus.db`)
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct CallusMcpServer {
    engine: Arc<LearningEngine<SqliteStore>>,
    embedder: Arc<dyn callus_core::Embedder>,
    scrubber: Arc<Scrubber>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchRequest {
    /// Error message, command snippet, or problem description
    query: String,
    /// Optional normalized tool name (e.g. `shell`, `edit`)
    tool_name: Option<String>,
    /// Caller OS (`windows`, `macos`, `linux`)
    os: Option<String>,
    /// Caller shell (`pwsh`, `cmd`, `bash`, `zsh`, `fish`)
    shell: Option<String>,
    /// Caller CLI (`claude-code`, `codex`, `antigravity`, `opencode`)
    cli: Option<String>,
    /// Active model identifier
    model: Option<String>,
    /// Repository hash for repo-scoped isolation and context boosting
    repo_hash: Option<String>,
    /// Maximum results to return (default 5)
    limit: Option<usize>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct SearchHitDto {
    pattern_id: String,
    fix_id: String,
    summary: String,
    score: f64,
    confidence: f64,
    worked_count: u64,
    failed_count: u64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct SearchResponse {
    hits: Vec<SearchHitDto>,
    vector_used: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReportRequest {
    /// Error message or failure description (scrubbed before storage)
    error_text: String,
    /// Actionable fix summary (<= 240 chars, scrubbed before storage)
    fix_summary: String,
    /// Optional tool name (default `shell`)
    tool_name: Option<String>,
    /// Optional failing command snippet
    pattern_before: Option<String>,
    /// Optional working replacement snippet
    pattern_after: Option<String>,
    /// Optional deterministic regex for a `pre_tool_guard`
    guard_regex: Option<String>,
    /// Scope (`personal`, `repo`, `team`, `public-pack`)
    scope: Option<String>,
    /// Optional repository hash when `scope = "repo"`
    repo_hash: Option<String>,
    os: Option<String>,
    shell: Option<String>,
    cli: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ReportResponse {
    pattern_id: String,
    fix_id: String,
    scope: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct FeedbackRequest {
    pattern_id: String,
    fix_id: String,
    /// True if the suggested fix resolved the issue, false otherwise
    worked: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct FeedbackResponse {
    pattern_id: String,
    fix_id: String,
    worked_count: u64,
    failed_count: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ExportCandidatesRequest {
    /// Filter scope: `repo`, `global`, or `all` (default `all`)
    scope: Option<String>,
    /// Optional repository hash for `repo` or `all` scope
    repo_hash: Option<String>,
    /// Age threshold in days for stale-rule prune detection (default 90)
    stale_days: Option<u64>,
    /// Max candidates returned (default 40)
    limit: Option<usize>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ExportCandidateDto {
    pattern_id: String,
    title: String,
    scope: String,
    occurrence_count: u64,
    last_seen_at: String,
    fix_id: String,
    fix_summary: String,
    worked_count: u64,
    failed_count: u64,
    impact_score: f64,
    stale: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ExportCandidatesResponse {
    candidates: Vec<ExportCandidateDto>,
}

impl From<ExportCandidate> for ExportCandidateDto {
    fn from(c: ExportCandidate) -> Self {
        Self {
            pattern_id: c.pattern_id,
            title: c.title,
            scope: c.scope.as_str().to_owned(),
            occurrence_count: c.occurrence_count,
            last_seen_at: c.last_seen_at,
            fix_id: c.fix_id,
            fix_summary: c.fix_summary,
            worked_count: c.worked_count,
            failed_count: c.failed_count,
            impact_score: c.impact_score,
            stale: c.stale,
        }
    }
}

#[tool_router(server_handler)]
impl CallusMcpServer {
    #[tool(
        name = "callus_search",
        description = "Hybrid vector + BM25 + knowledge-graph lookup for proven fixes matching an error message or task context."
    )]
    async fn callus_search(
        &self,
        Parameters(req): Parameters<SearchRequest>,
    ) -> Result<Json<SearchResponse>, String> {
        let scrubbed_query = self.scrubber.scrub_text(&req.query).scrubbed;
        let query_vec = self
            .embedder
            .embed(&scrubbed_query)
            .await
            .map_err(|e| e.to_string())?;
        let vector_used = query_vec.is_some();

        let template = normalize_error_template(&scrubbed_query);
        let exact_sig = req
            .tool_name
            .as_deref()
            .map(|tool| compute_signature_hash(tool, &template));

        let context = build_context(
            req.os.as_deref(),
            req.shell.as_deref(),
            req.cli.as_deref(),
            req.model,
            req.repo_hash,
        );
        let search = SearchQuery {
            text: scrubbed_query,
            tool_name: req.tool_name,
            context,
            limit: req.limit.unwrap_or(5).clamp(1, 50),
        };
        let ranked = self
            .engine
            .search_fixes(
                &search,
                query_vec.as_deref(),
                exact_sig.as_deref(),
                &now_iso_utc(),
                &HashSet::new(),
            )
            .await
            .map_err(|e| e.to_string())?;

        let hits = ranked
            .into_iter()
            .map(|r| SearchHitDto {
                pattern_id: r.pattern_id,
                fix_id: r.fix.id,
                summary: r.fix.summary,
                score: r.score,
                confidence: r.confidence,
                worked_count: r.worked_count,
                failed_count: r.failed_count,
            })
            .collect();
        Ok(Json(SearchResponse { hits, vector_used }))
    }

    #[tool(
        name = "callus_report",
        description = "Explicitly record a scrubbed mistake pattern and its verified fix when hooks could not observe it."
    )]
    async fn callus_report(
        &self,
        Parameters(req): Parameters<ReportRequest>,
    ) -> Result<Json<ReportResponse>, String> {
        let now = now_iso_utc();
        let tool_name = req.tool_name.unwrap_or_else(|| "shell".to_owned());
        let scrubbed_err = self.scrubber.scrub_text(&req.error_text).scrubbed;
        let scrubbed_summary = self.scrubber.scrub_text(&req.fix_summary).scrubbed;
        let before = req
            .pattern_before
            .map(|s| self.scrubber.scrub_text(&s).scrubbed);
        let after = req
            .pattern_after
            .map(|s| self.scrubber.scrub_text(&s).scrubbed);

        let template = normalize_error_template(&scrubbed_err);
        let sig = compute_signature_hash(&tool_name, &template);
        let scope = match req.scope.as_deref() {
            Some(raw) => raw.parse::<Scope>().map_err(|e| e.to_string())?,
            None => {
                if req.repo_hash.is_some() {
                    Scope::Repo
                } else {
                    Scope::Team
                }
            }
        };

        let pattern = self
            .engine
            .store()
            .upsert_pattern(&ErrorPattern {
                id: format!("pat-{}", &sig[..16]),
                signature_hash: sig,
                title: template.chars().take(120).collect(),
                error_template: template,
                tool_name,
                scope,
                repo_hash: req.repo_hash.clone(),
                occurrence_count: 1,
                first_seen_at: now.clone(),
                last_seen_at: now.clone(),
                embedding_status: EmbeddingStatus::Pending,
                updated_at: now.clone(),
            })
            .await
            .map_err(|e| e.to_string())?;

        if let Ok(Some(vec)) = self.embedder.embed(&pattern.error_template).await {
            let _ = self
                .engine
                .store()
                .upsert_pattern_embedding(&pattern.id, &vec)
                .await;
        }

        let fix_sig = compute_signature_hash(&pattern.id, &scrubbed_summary);
        let fix_type = if req.guard_regex.is_some() {
            FixType::PreToolGuard
        } else {
            FixType::Hint
        };
        let fix = self
            .engine
            .store()
            .upsert_fix(&Fix {
                id: format!("fix-{}", &fix_sig[..16]),
                summary: scrubbed_summary,
                fix_type,
                pattern_before: before,
                pattern_after: after,
                guard_regex: req.guard_regex,
                status: FixStatus::Verified,
                scope,
                source: FixSource::AgentReport,
                created_at: now.clone(),
                updated_at: now.clone(),
            })
            .await
            .map_err(|e| e.to_string())?;

        let err_node = format!("err:{}", pattern.id);
        let fix_node = format!("fix:{}", fix.id);
        for node in [
            GraphNode {
                id: err_node.clone(),
                kind: NodeKind::ErrorPattern,
                label: pattern.title.clone(),
                props_json: json!({}),
            },
            GraphNode {
                id: fix_node.clone(),
                kind: NodeKind::Fix,
                label: fix.summary.clone(),
                props_json: json!({"source": "agent_report"}),
            },
        ] {
            self.engine
                .store()
                .upsert_node(&node)
                .await
                .map_err(|e| e.to_string())?;
        }
        self.engine
            .store()
            .record_edge_feedback(&err_node, EdgeType::ResolvedBy, &fix_node, 1, 0, 1, &now)
            .await
            .map_err(|e| e.to_string())?;

        self.attach_optional_context(
            &err_node,
            req.os.as_deref(),
            req.shell.as_deref(),
            req.cli.as_deref(),
            req.model.as_deref(),
            req.repo_hash.as_deref(),
            &now,
        )
        .await
        .map_err(|e| e.to_string())?;

        Ok(Json(ReportResponse {
            pattern_id: pattern.id,
            fix_id: fix.id,
            scope: scope.as_str().to_owned(),
        }))
    }

    #[tool(
        name = "callus_feedback",
        description = "Record whether a suggested Callus fix worked or failed to update closed-loop Wilson confidence."
    )]
    async fn callus_feedback(
        &self,
        Parameters(req): Parameters<FeedbackRequest>,
    ) -> Result<Json<FeedbackResponse>, String> {
        let (worked_delta, failed_delta) = if req.worked { (1, 0) } else { (0, 1) };
        let edge = self
            .engine
            .store()
            .record_edge_feedback(
                &format!("err:{}", req.pattern_id),
                EdgeType::ResolvedBy,
                &format!("fix:{}", req.fix_id),
                worked_delta,
                failed_delta,
                1,
                &now_iso_utc(),
            )
            .await
            .map_err(|e| e.to_string())?;

        Ok(Json(FeedbackResponse {
            pattern_id: req.pattern_id,
            fix_id: req.fix_id,
            worked_count: edge.worked_count,
            failed_count: edge.failed_count,
        }))
    }

    #[tool(
        name = "callus_export_candidates",
        description = "Return ranked recurring error patterns, fixes, and stale prune candidates for AGENTS.md / CLAUDE.md updates."
    )]
    async fn callus_export_candidates(
        &self,
        Parameters(req): Parameters<ExportCandidatesRequest>,
    ) -> Result<Json<ExportCandidatesResponse>, String> {
        let scope = match req.scope.as_deref().unwrap_or("all") {
            "repo" => ExportScope::Repo,
            "global" => ExportScope::Global,
            "all" => ExportScope::All,
            other => return Err(format!("invalid scope {other:?}; use repo, global, or all")),
        };
        let mut items = collect_candidates(
            self.engine.store(),
            scope,
            req.repo_hash.as_deref(),
            req.stale_days.unwrap_or(90),
            &now_iso_utc(),
        )
        .await
        .map_err(|e| e.to_string())?;
        items.truncate(req.limit.unwrap_or(40).clamp(1, 200));
        Ok(Json(ExportCandidatesResponse {
            candidates: items.into_iter().map(ExportCandidateDto::from).collect(),
        }))
    }
}

impl CallusMcpServer {
    #[expect(clippy::too_many_arguments, reason = "optional context attributes")]
    async fn attach_optional_context(
        &self,
        err_node: &str,
        os: Option<&str>,
        shell: Option<&str>,
        cli: Option<&str>,
        model: Option<&str>,
        repo_hash: Option<&str>,
        now: &str,
    ) -> Result<(), callus_store::StoreError> {
        let mut nodes = Vec::new();
        if let (Some(o), Some(s)) = (os, shell) {
            nodes.push(GraphNode {
                id: format!("env:{o}:{s}"),
                kind: NodeKind::Environment,
                label: format!("{o}/{s}"),
                props_json: json!({"os": o, "shell": s}),
            });
        }
        if let Some(c) = cli {
            nodes.push(GraphNode {
                id: format!("cli:{c}"),
                kind: NodeKind::Cli,
                label: c.to_owned(),
                props_json: json!({}),
            });
        }
        if let Some(m) = model {
            nodes.push(GraphNode {
                id: format!("model:{m}"),
                kind: NodeKind::Model,
                label: m.to_owned(),
                props_json: json!({}),
            });
        }
        if let Some(r) = repo_hash {
            nodes.push(GraphNode {
                id: format!("repo:{r}"),
                kind: NodeKind::Repo,
                label: r.to_owned(),
                props_json: json!({}),
            });
        }
        for node in nodes {
            let dst = node.id.clone();
            self.engine.store().upsert_node(&node).await?;
            self.engine
                .store()
                .record_edge_feedback(err_node, EdgeType::OccursIn, &dst, 0, 0, 1, now)
                .await?;
        }
        Ok(())
    }
}

fn build_context(
    os: Option<&str>,
    shell: Option<&str>,
    cli: Option<&str>,
    model: Option<String>,
    repo_hash: Option<String>,
) -> Option<ContextInfo> {
    if os.is_none() && shell.is_none() && cli.is_none() && model.is_none() && repo_hash.is_none() {
        return None;
    }
    Some(ContextInfo {
        os: os
            .and_then(|s| s.parse().ok())
            .unwrap_or(if cfg!(target_os = "windows") {
                OsKind::Windows
            } else if cfg!(target_os = "macos") {
                OsKind::MacOs
            } else {
                OsKind::Linux
            }),
        shell: shell
            .and_then(|s| s.parse().ok())
            .unwrap_or(ShellKind::Unknown),
        cli: cli
            .and_then(|s| s.parse().ok())
            .unwrap_or(CliKind::ClaudeCode),
        model,
        repo_hash,
    })
}

pub(crate) fn run(args: Args) -> Result<(), CallusError> {
    let db_path = resolve_db_path(args.db);
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| CallusError::Io {
            context: format!("cannot create {}", parent.display()),
            source,
        })?;
    }

    let embed_cfg = EmbeddingConfig::from_env().unwrap_or_default();
    let dims = if embed_cfg.provider == callus_core::EmbeddingProvider::OpenAiCompatible {
        embed_cfg.dims
    } else {
        configured_dims()
    };
    let embedder = build_embedder(&embed_cfg).map_err(|e| CallusError::Io {
        context: "embedder configuration failed".to_owned(),
        source: std::io::Error::other(e.to_string()),
    })?;
    let store = SqliteStore::open(&db_path, dims).map_err(|e| CallusError::Io {
        context: "cannot open sqlite store".to_owned(),
        source: std::io::Error::other(e.to_string()),
    })?;
    let scrubber = Scrubber::new(&ScrubberConfig {
        repo_root: std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
        home_dir: std::env::home_dir().map(|p| p.to_string_lossy().into_owned()),
        extra_hostnames: Vec::new(),
        deny_regexes: Vec::new(),
    })?;

    let server = CallusMcpServer {
        engine: Arc::new(LearningEngine::new(store, EngineConfig::default())),
        embedder,
        scrubber: Arc::new(scrubber),
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|source| CallusError::Io {
            context: "cannot start tokio runtime".to_owned(),
            source,
        })?;

    rt.block_on(async move {
        let running = server
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        running
            .waiting()
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok::<(), std::io::Error>(())
    })
    .map_err(|source| CallusError::Io {
        context: "MCP stdio server exited with error".to_owned(),
        source,
    })
}
