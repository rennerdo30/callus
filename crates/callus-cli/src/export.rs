//! `callus export`: tiered compiler generating token-budgeted `AGENTS.md` /
//! `CLAUDE.md` / `GEMINI.md` diffs, deterministic `PreToolUse` guards, and
//! anonymized community rule packs.

use std::{fmt::Write as _, path::PathBuf};

use callus_core::{CallusError, ErrorPattern, Fix, FixStatus, FixType, Scope};
use callus_store::{KnowledgeStore, SqliteStore, effective_confidence, parse_iso_epoch_secs};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    adapters::{compute_repo_hash, now_iso_utc},
    hook::{configured_dims, resolve_db_path},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ExportFormat {
    AgentsMd,
    ClaudeMd,
    GeminiMd,
    Hooks,
    Pack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ExportScope {
    /// Export only pitfalls bound to the current repository
    Repo,
    /// Export cross-repo / OS / shell / agent-wide pitfalls
    Global,
    /// Export both repository and global pitfalls
    All,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Output format
    #[arg(long, value_enum, default_value = "agents-md")]
    format: ExportFormat,
    /// Scope filter (`repo` inside a git repo, or `global` / `all`)
    #[arg(long, value_enum)]
    scope: Option<ExportScope>,
    /// Explicit repository hash override (defaults to current git repo)
    #[arg(long, value_name = "HASH")]
    repo_hash: Option<String>,
    /// Maximum rule lines emitted for markdown formats (prevents startup bloat)
    #[arg(long, default_value_t = 40)]
    budget_lines: usize,
    /// Age threshold in days after which unused rules become prune candidates
    #[arg(long, default_value_t = 90)]
    stale_days: u64,
    /// Existing markdown rule file to compare against when computing prune suggestions
    #[arg(long, value_name = "PATH")]
    existing: Option<PathBuf>,
    /// Override local SQLite database path
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
}

/// Ranked candidate returned by both `callus export` and MCP `callus_export_candidates`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExportCandidate {
    pub pattern_id: String,
    pub title: String,
    pub scope: Scope,
    pub repo_hash: Option<String>,
    pub occurrence_count: u64,
    pub last_seen_at: String,
    pub fix_id: String,
    pub fix_summary: String,
    pub fix_status: FixStatus,
    pub fix_type: FixType,
    pub guard_regex: Option<String>,
    pub worked_count: u64,
    pub failed_count: u64,
    pub confidence: f64,
    pub impact_score: f64,
    pub stale: bool,
}

pub(crate) fn run(args: Args) -> Result<(), CallusError> {
    let db_path = resolve_db_path(args.db);
    let store = SqliteStore::open(&db_path, configured_dims()).map_err(|e| store_io_err(&e))?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|e| CallusError::Io {
            context: "cannot start tokio runtime".to_owned(),
            source: e,
        })?;

    let detected_repo = args.repo_hash.or_else(|| {
        std::env::current_dir()
            .ok()
            .as_deref()
            .and_then(compute_repo_hash)
    });
    let scope = args.scope.unwrap_or_else(|| {
        if detected_repo.is_some() {
            ExportScope::Repo
        } else {
            ExportScope::Global
        }
    });

    let now_iso = now_iso_utc();
    let candidates = rt
        .block_on(collect_candidates(
            &store,
            scope,
            detected_repo.as_deref(),
            args.stale_days,
            &now_iso,
        ))
        .map_err(|e| store_io_err(&e))?;

    let existing_md = match &args.existing {
        Some(path) => Some(
            std::fs::read_to_string(path).map_err(|source| CallusError::Io {
                context: format!("cannot read {}", path.display()),
                source,
            })?,
        ),
        None => None,
    };

    let output = match args.format {
        ExportFormat::AgentsMd => render_markdown(
            "AGENTS.md",
            scope,
            &candidates,
            args.budget_lines,
            existing_md.as_deref(),
        ),
        ExportFormat::ClaudeMd => render_markdown(
            "CLAUDE.md",
            scope,
            &candidates,
            args.budget_lines,
            existing_md.as_deref(),
        ),
        ExportFormat::GeminiMd => render_markdown(
            "GEMINI.md",
            scope,
            &candidates,
            args.budget_lines,
            existing_md.as_deref(),
        ),
        ExportFormat::Hooks => {
            let guards = rt
                .block_on(store.list_active_guards(detected_repo.as_deref()))
                .map_err(|e| store_io_err(&e))?;
            render_hooks_json(&guards)?
        }
        ExportFormat::Pack => render_pack_json(&candidates, &now_iso)?,
    };

    print!("{output}");
    Ok(())
}

/// Collects and ranks export candidates by `frequency * impact * confidence`.
pub(crate) async fn collect_candidates<S: KnowledgeStore>(
    store: &S,
    scope: ExportScope,
    repo_hash: Option<&str>,
    stale_days: u64,
    now_iso: &str,
) -> Result<Vec<ExportCandidate>, callus_store::StoreError> {
    let patterns = store.list_patterns(None, None, 200).await?;
    let mut out = Vec::new();

    for pattern in patterns {
        if !matches_export_scope(&pattern, scope, repo_hash) {
            continue;
        }
        let fixes = store.fixes_for_pattern(&pattern.id).await?;
        let Some(best) = fixes.into_iter().next() else {
            continue;
        };

        let confidence = effective_confidence(
            best.fix.status,
            best.edge.worked_count,
            best.edge.failed_count,
        );
        let freq =
            u32::try_from(pattern.occurrence_count.max(1)).map_or(f64::from(u32::MAX), f64::from);
        let impact = u32::try_from(best.edge.worked_count.saturating_add(1))
            .map_or(f64::from(u32::MAX), f64::from);
        let impact_score = freq * impact * confidence.max(0.05);
        let stale = is_stale(&pattern.last_seen_at, now_iso, stale_days)
            || matches!(best.fix.status, FixStatus::Deprecated | FixStatus::Obsolete);

        out.push(ExportCandidate {
            pattern_id: pattern.id,
            title: pattern.title,
            scope: pattern.scope,
            repo_hash: pattern.repo_hash,
            occurrence_count: pattern.occurrence_count,
            last_seen_at: pattern.last_seen_at,
            fix_id: best.fix.id,
            fix_summary: best.fix.summary,
            fix_status: best.fix.status,
            fix_type: best.fix.fix_type,
            guard_regex: best.fix.guard_regex,
            worked_count: best.edge.worked_count,
            failed_count: best.edge.failed_count,
            confidence,
            impact_score,
            stale,
        });
    }

    out.sort_by(|a, b| {
        a.stale
            .cmp(&b.stale)
            .then_with(|| b.impact_score.total_cmp(&a.impact_score))
            .then_with(|| a.pattern_id.cmp(&b.pattern_id))
    });
    Ok(out)
}

fn matches_export_scope(
    pattern: &ErrorPattern,
    scope: ExportScope,
    repo_hash: Option<&str>,
) -> bool {
    match scope {
        ExportScope::Repo => {
            pattern.scope == Scope::Repo
                && repo_hash.is_some()
                && pattern.repo_hash.as_deref() == repo_hash
        }
        ExportScope::Global => pattern.scope != Scope::Repo,
        ExportScope::All => {
            pattern.scope != Scope::Repo
                || (repo_hash.is_some() && pattern.repo_hash.as_deref() == repo_hash)
        }
    }
}

fn is_stale(last_seen_iso: &str, now_iso: &str, stale_days: u64) -> bool {
    let (Some(last), Some(now)) = (
        parse_iso_epoch_secs(last_seen_iso),
        parse_iso_epoch_secs(now_iso),
    ) else {
        return false;
    };
    let threshold = i64::try_from(stale_days.saturating_mul(86_400)).unwrap_or(i64::MAX);
    now.saturating_sub(last) >= threshold
}

fn render_markdown(
    target_file: &str,
    scope: ExportScope,
    candidates: &[ExportCandidate],
    budget_lines: usize,
    existing_md: Option<&str>,
) -> String {
    let scope_label = match scope {
        ExportScope::Repo => "repository",
        ExportScope::Global => "global",
        ExportScope::All => "repository + global",
    };

    let active: Vec<&ExportCandidate> = candidates
        .iter()
        .filter(|c| !c.stale)
        .take(budget_lines)
        .collect();
    let stale: Vec<&ExportCandidate> = candidates.iter().filter(|c| c.stale).collect();

    let mut out = String::new();
    let _ = writeln!(
        out,
        "--- a/{target_file}\n+++ b/{target_file}\n@@ Callus learned rules ({scope_label}, budget {budget_lines} lines) @@"
    );
    if active.is_empty() {
        let _ = writeln!(out, "# No active learned rules match this scope yet.");
    } else {
        for c in active {
            let marker = format!("callus:{}", c.pattern_id);
            let already_present = existing_md.is_some_and(|md| md.contains(&marker));
            let prefix = if already_present { " " } else { "+" };
            let total = c.worked_count + c.failed_count;
            let _ = writeln!(
                out,
                "{prefix}- {} (seen {}x, worked {}/{}) <!-- {marker} -->",
                c.fix_summary, c.occurrence_count, c.worked_count, total
            );
        }
    }

    if !stale.is_empty() {
        let _ = writeln!(out, "\n### Suggested Stale-Rule Prunes");
        for c in stale {
            let marker = format!("callus:{}", c.pattern_id);
            let present_note = if existing_md.is_some_and(|md| md.contains(&marker)) {
                " [present in file]"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "-- {} (last seen {}, status {}){present_note} <!-- {marker} -->",
                c.fix_summary,
                c.last_seen_at,
                c.fix_status.as_str()
            );
        }
    }
    out
}

fn render_hooks_json(guards: &[Fix]) -> Result<String, CallusError> {
    let entries: Vec<_> = guards
        .iter()
        .map(|g| {
            json!({
                "id": g.id,
                "summary": g.summary,
                "guard_regex": g.guard_regex,
                "scope": g.scope.as_str(),
                "status": g.status.as_str(),
            })
        })
        .collect();
    let mut rendered = serde_json::to_string_pretty(&json!({
        "schema_version": 1,
        "pre_tool_guards": entries,
    }))?;
    rendered.push('\n');
    Ok(rendered)
}

fn render_pack_json(candidates: &[ExportCandidate], now_iso: &str) -> Result<String, CallusError> {
    // Public packs NEVER include personal-scoped rules.
    let items: Vec<_> = candidates
        .iter()
        .filter(|c| !c.stale && c.scope != Scope::Personal)
        .map(|c| {
            json!({
                "pattern_id": c.pattern_id,
                "title": c.title,
                "fix_summary": c.fix_summary,
                "fix_type": c.fix_type.as_str(),
                "guard_regex": c.guard_regex,
                "worked_count": c.worked_count,
                "failed_count": c.failed_count,
                "scope": "public-pack",
            })
        })
        .collect();
    let mut rendered = serde_json::to_string_pretty(&json!({
        "schema_version": 1,
        "generated_at": now_iso,
        "rules": items,
    }))?;
    rendered.push('\n');
    Ok(rendered)
}

fn store_io_err(err: &callus_store::StoreError) -> CallusError {
    CallusError::Io {
        context: "storage error".to_owned(),
        source: std::io::Error::other(err.to_string()),
    }
}
