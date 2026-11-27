//! Deterministic hybrid ranking math: RRF, graph context multipliers,
//! Wilson score lower bound, and exponential recency decay.

use callus_core::{ContextInfo, FixStatus, GraphNode, NodeKind};

/// Default Reciprocal Rank Fusion constant $k$.
pub const RRF_K: f64 = 60.0;
/// Default Wilson score $z$-value (95% confidence interval).
pub const WILSON_Z: f64 = 1.96;
/// Default exponential recency half-life in days.
pub const DEFAULT_HALF_LIFE_DAYS: f64 = 45.0;
/// Default confidence gate for live `PostToolUseFailure` hint injection.
pub const DEFAULT_MIN_INJECT_CONFIDENCE: f64 = 0.45;

/// Computes normalized relevance in `[0.0, 1.0]` from exact signature match,
/// BM25 1-based rank, and optional vector 1-based rank.
pub fn hybrid_relevance(
    exact_signature: bool,
    bm25_rank: Option<usize>,
    vec_rank: Option<usize>,
) -> f64 {
    let rrf_term = |rank: Option<usize>| -> f64 {
        rank.map_or(0.0, |r| {
            let r_f = u32::try_from(r.max(1)).map_or(f64::from(u32::MAX), f64::from);
            1.0 / (RRF_K + r_f)
        })
    };

    let base_unit = 1.0 / (RRF_K + 1.0);
    let rrf_raw = rrf_term(bm25_rank) + rrf_term(vec_rank);
    let rrf_norm = (rrf_raw / base_unit).min(1.0);
    let exact_bonus = if exact_signature { 0.55 } else { 0.0 };
    (rrf_norm * 0.65 + exact_bonus).min(1.0)
}

/// Wilson score lower bound ($z = 1.96$) for `worked` successes out of `worked + failed`.
pub fn wilson_lower_bound(worked: u64, failed: u64) -> f64 {
    let total = worked.saturating_add(failed);
    if total == 0 {
        return 0.0;
    }
    let n = u32::try_from(total).map_or(f64::from(u32::MAX), f64::from);
    let w = u32::try_from(worked).map_or(f64::from(u32::MAX), f64::from);
    let p = w / n;
    let z2 = WILSON_Z * WILSON_Z;
    let denom = 1.0 + z2 / n;
    let center = p + z2 / (2.0 * n);
    let margin = WILSON_Z * ((p * (1.0 - p) + z2 / (4.0 * n)) / n).sqrt();
    ((center - margin) / denom).clamp(0.0, 1.0)
}

/// Effective confidence combining Wilson score with status prior so human-verified
/// fixes and clean first-time auto-recoveries can clear the injection gate while
/// any failed retry immediately drops confidence.
pub fn effective_confidence(status: FixStatus, worked: u64, failed: u64) -> f64 {
    match status {
        FixStatus::Deprecated | FixStatus::Obsolete => 0.0,
        FixStatus::Verified => wilson_lower_bound(worked.saturating_add(4), failed),
        FixStatus::Candidate => {
            let w = wilson_lower_bound(worked, failed);
            if failed == 0 && worked > 0 {
                0.35 + 0.65 * w
            } else {
                w
            }
        }
    }
}

/// Exponential recency decay with configurable `half_life_days` (default 45d).
pub fn recency_decay(last_seen_iso: &str, now_iso: &str, half_life_days: f64) -> f64 {
    if half_life_days <= 0.0 {
        return 1.0;
    }
    let (Some(last), Some(now)) = (
        parse_iso_epoch_secs(last_seen_iso),
        parse_iso_epoch_secs(now_iso),
    ) else {
        return 1.0;
    };
    let delta_secs = now.saturating_sub(last);
    let age_days = f64::from(u32::try_from(delta_secs).unwrap_or(u32::MAX)) / 86_400.0;
    0.5_f64.powf(age_days / half_life_days)
}

/// Graph context multiplier comparing caller context against `OCCURS_IN` nodes.
pub fn context_multiplier(
    caller: Option<&ContextInfo>,
    pattern_nodes: &[GraphNode],
    is_shell_tool: bool,
) -> f64 {
    let Some(ctx) = caller else {
        return 1.0;
    };
    let mut mult = 1.0;

    let caller_env_id = format!("env:{}:{}", ctx.os.as_str(), ctx.shell.as_str());
    let caller_os_prefix = format!("env:{}:", ctx.os.as_str());
    let env_nodes: Vec<&GraphNode> = pattern_nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Environment)
        .collect();

    if !env_nodes.is_empty() {
        if env_nodes.iter().any(|n| n.id == caller_env_id) {
            mult *= 1.4;
        } else if is_shell_tool
            && !env_nodes
                .iter()
                .any(|n| n.id.starts_with(&caller_os_prefix))
        {
            // Penalize cross-OS shell fixes (e.g. do not inject a zsh fix on pwsh).
            mult *= 0.3;
        }
    }

    if let Some(repo) = &ctx.repo_hash {
        let repo_id = format!("repo:{repo}");
        if pattern_nodes.iter().any(|n| n.id == repo_id) {
            mult *= 1.3;
        }
    }

    let cli_id = format!("cli:{}", ctx.cli.as_str());
    let model_id = ctx.model.as_deref().map(|m| format!("model:{m}"));
    let cli_or_model_match = pattern_nodes
        .iter()
        .any(|n| n.id == cli_id || model_id.as_deref().is_some_and(|m| n.id == m));
    if cli_or_model_match {
        mult *= 1.15;
    }

    mult
}

/// Parses an ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ` or fractional) into Unix seconds.
pub fn parse_iso_epoch_secs(ts: &str) -> Option<i64> {
    let bytes = ts.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year: i64 = ts.get(0..4)?.parse().ok()?;
    let month: i64 = ts.get(5..7)?.parse().ok()?;
    let day: i64 = ts.get(8..10)?.parse().ok()?;
    let hour: i64 = ts.get(11..13)?.parse().ok()?;
    let min: i64 = ts.get(14..16)?.parse().ok()?;
    let sec: i64 = ts.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&min)
        || !(0..=60).contains(&sec)
    {
        return None;
    }
    // Howard Hinnant's civil-from-days algorithm (UTC).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = month;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + min * 60 + sec)
}
