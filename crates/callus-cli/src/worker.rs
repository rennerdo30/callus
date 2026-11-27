//! Background consolidation worker for `callus serve`:
//! - Backfills pending embeddings against external `llama.cpp` (`/v1/embeddings`)
//! - Auto-promotes generic patterns observed in >= 2 distinct repos to `team`
//! - Marks repeatedly failing or stale candidate fixes as `obsolete`
//! - Prunes local short-retention episodes older than 7 days

use std::{sync::Arc, time::Duration};

use callus_core::{Embedder, FixStatus, Scope};
use callus_store::{KnowledgeStore, parse_iso_epoch_secs};
use serde::Serialize;

use crate::adapters::{iso_utc_days_ago, now_iso_utc};

/// Summary of one background consolidation pass.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ConsolidationReport {
    pub backfilled_embeddings: usize,
    pub promoted_patterns: usize,
    pub obsoleted_fixes: usize,
    pub pruned_episodes: usize,
}

/// Runs one consolidation sweep without ever crashing the server on transient embedder failures.
pub(crate) async fn consolidate_once(
    store: &Arc<dyn KnowledgeStore>,
    embedder: &Arc<dyn Embedder>,
    stale_days: u64,
    now_iso: &str,
) -> ConsolidationReport {
    let mut report = ConsolidationReport::default();

    // 1. Backfill pending embeddings when external `llama.cpp` is enabled and reachable.
    if embedder.is_enabled()
        && let Ok(pending) = store.list_pending_embeddings(32).await
        && !pending.is_empty()
    {
        let inputs: Vec<String> = pending
            .iter()
            .map(|p| format!("{} {}", p.title, p.error_template))
            .collect();
        if let Ok(vectors) = embedder.embed_batch_strict(&inputs).await {
            for (pat, vec) in pending.iter().zip(vectors.iter()) {
                if store.upsert_pattern_embedding(&pat.id, vec).await.is_ok() {
                    report.backfilled_embeddings += 1;
                }
            }
        }
    }

    // 2. Auto-promote generic repo patterns observed in >= 2 distinct repos & sweep stale fixes.
    let stale_cutoff_secs = i64::try_from(stale_days.saturating_mul(86_400)).unwrap_or(i64::MAX);
    let now_secs = parse_iso_epoch_secs(now_iso);

    if let Ok(patterns) = store.list_patterns(None, None, 200).await {
        for pat in patterns {
            if pat.scope == Scope::Repo
                && !pat.error_template.contains("<REPO>")
                && store.pattern_repo_count(&pat.id).await.unwrap_or(0) >= 2
                && store
                    .update_pattern_scope(&pat.id, Scope::Team, None, now_iso)
                    .await
                    .is_ok()
            {
                report.promoted_patterns += 1;
            }

            let pattern_stale = now_secs
                .zip(parse_iso_epoch_secs(&pat.last_seen_at))
                .is_some_and(|(now, last)| now.saturating_sub(last) >= stale_cutoff_secs);

            if let Ok(fixes) = store.fixes_for_pattern(&pat.id).await {
                for cand in fixes {
                    if cand.fix.status != FixStatus::Candidate {
                        continue;
                    }
                    let heavily_failing = cand.edge.failed_count >= 3
                        && cand.edge.failed_count > cand.edge.worked_count.saturating_mul(2);
                    if (pattern_stale || heavily_failing)
                        && store
                            .update_fix_status(&cand.fix.id, FixStatus::Obsolete, now_iso)
                            .await
                            .is_ok()
                    {
                        report.obsoleted_fixes += 1;
                    }
                }
            }
        }
    }

    // 3. Prune expired short-retention episodes (> 7 days).
    if let Ok(pruned) = store.prune_episodes_before(&iso_utc_days_ago(7)).await {
        report.pruned_episodes = pruned;
    }

    report
}

/// Spawns the periodic background consolidation loop inside `callus serve`.
pub(crate) fn spawn_worker(
    store: Arc<dyn KnowledgeStore>,
    embedder: Arc<dyn Embedder>,
    interval_secs: u64,
) {
    if interval_secs == 0 {
        return;
    }
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            ticker.tick().await;
            let _ = consolidate_once(&store, &embedder, 90, &now_iso_utc()).await;
        }
    });
}
