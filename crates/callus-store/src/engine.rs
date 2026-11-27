//! Self-learning engine: Failure -> Recovery pairing, closed-loop feedback,
//! smart repo-vs-global scope classification, and hybrid retrieval.

use std::collections::{HashMap, HashSet};

use callus_core::{
    ContextInfo, EdgeType, EmbeddingStatus, Episode, ErrorPattern, Fix, FixSource, FixStatus,
    FixType, GraphNode, NodeKind, RankedFix, Scope, Scrubber, SearchQuery, ToolEvent, ToolPhase,
    compute_signature_hash, normalize_error_template,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    KnowledgeStore, StoreError,
    ranking::{
        DEFAULT_HALF_LIFE_DAYS, DEFAULT_MIN_INJECT_CONFIDENCE, context_multiplier,
        effective_confidence, hybrid_relevance, parse_iso_epoch_secs, recency_decay,
    },
};

/// Tunable parameters for the self-learning engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Maximum recent session episodes inspected for Failure -> Recovery pairing.
    pub recovery_max_turns: usize,
    /// Maximum elapsed seconds between failure and recovery in one session.
    pub recovery_max_window_secs: i64,
    /// Minimum hybrid score required to inject a hint on `PostToolUseFailure`.
    pub min_inject_confidence: f64,
    /// Exponential recency half-life in days.
    pub recency_half_life_days: f64,
    /// Distinct repositories required before auto-promoting a `repo` pattern to `team`.
    pub repo_promotion_threshold: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            recovery_max_turns: 3,
            recovery_max_window_secs: 180,
            min_inject_confidence: DEFAULT_MIN_INJECT_CONFIDENCE,
            recency_half_life_days: DEFAULT_HALF_LIFE_DAYS,
            repo_promotion_threshold: 2,
        }
    }
}

/// Outcome returned after ingesting one normalized [`ToolEvent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IngestOutcome {
    /// `PreToolUse` allowed the tool call to proceed.
    Allowed,
    /// `PreToolUse` matched an enabled deterministic guard rule.
    GuardTriggered { fix: Box<Fix> },
    /// `PostToolUseFailure` recorded the failure with no fix above the injection gate.
    RecordedFailure { pattern: Box<ErrorPattern> },
    /// `PostToolUseFailure` matched a high-confidence fix to inject into agent context.
    HintInjected {
        pattern: Box<ErrorPattern>,
        ranked: Box<RankedFix>,
    },
    /// `PostToolUse` succeeded and paired a recovery or confirmed an injected hint.
    Recovered {
        pattern_id: String,
        fix_id: String,
        confirmed_injected: bool,
    },
    /// `PostToolUse` succeeded with no preceding unpaired failure in the window.
    RecordedOk,
}

/// Coordinates scrubbing, storage, learning, feedback, and hybrid ranking.
#[derive(Debug, Clone)]
pub struct LearningEngine<S> {
    store: S,
    config: EngineConfig,
}

impl<S: KnowledgeStore> LearningEngine<S> {
    pub const fn new(store: S, config: EngineConfig) -> Self {
        Self { store, config }
    }

    pub const fn store(&self) -> &S {
        &self.store
    }

    pub const fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Scrubs and processes one normalized hook event.
    ///
    /// `query_embedding` is optional (the `<5ms` hook hot path passes `None` so hooks
    /// never stall on an external embedder; MCP/API queries may pass a computed vector).
    pub async fn ingest_event(
        &self,
        raw_event: &ToolEvent,
        scrubber: &Scrubber,
        query_embedding: Option<&[f32]>,
    ) -> Result<IngestOutcome, StoreError> {
        let mut event = raw_event.clone();
        scrubber.scrub_event(&mut event);

        match event.phase {
            ToolPhase::Pre => self.handle_pre(&event).await,
            ToolPhase::PostFailure => self.handle_failure(&event, query_embedding).await,
            ToolPhase::PostOk => self.handle_ok(&event).await,
        }
    }

    async fn handle_pre(&self, event: &ToolEvent) -> Result<IngestOutcome, StoreError> {
        let Some(input) = event.command_or_input.as_deref() else {
            return Ok(IngestOutcome::Allowed);
        };
        let guards = self
            .store
            .list_active_guards(event.repo_hash.as_deref())
            .await?;
        for guard in guards {
            let Some(regex_src) = guard.guard_regex.as_deref() else {
                continue;
            };
            if let Ok(re) = Regex::new(regex_src)
                && re.is_match(input)
            {
                return Ok(IngestOutcome::GuardTriggered {
                    fix: Box::new(guard),
                });
            }
        }
        Ok(IngestOutcome::Allowed)
    }

    async fn handle_failure(
        &self,
        event: &ToolEvent,
        query_embedding: Option<&[f32]>,
    ) -> Result<IngestOutcome, StoreError> {
        let raw_error = event
            .error_text
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or(event.command_or_input.as_deref())
            .unwrap_or("unknown tool failure");
        let error_template = normalize_error_template(raw_error);
        let signature_hash = compute_signature_hash(&event.tool_name, &error_template);

        let initial_scope = if event.repo_hash.is_some() {
            Scope::Repo
        } else {
            Scope::Personal
        };

        let pattern_id = format!("pat-{}", &signature_hash[..16]);
        let candidate_pattern = ErrorPattern {
            id: pattern_id,
            signature_hash: signature_hash.clone(),
            title: summarize_title(&error_template),
            error_template: error_template.clone(),
            tool_name: event.tool_name.clone(),
            scope: initial_scope,
            repo_hash: event.repo_hash.clone(),
            occurrence_count: 1,
            first_seen_at: event.timestamp.clone(),
            last_seen_at: event.timestamp.clone(),
            embedding_status: EmbeddingStatus::Pending,
            updated_at: event.timestamp.clone(),
        };
        let mut pattern = self.store.upsert_pattern(&candidate_pattern).await?;
        self.record_context_graph(&pattern, event).await?;
        self.maybe_promote_pattern(&mut pattern, event).await?;
        let suppressed_fixes = self
            .collect_suppressed_and_penalize(&pattern, event)
            .await?;

        let query = SearchQuery {
            text: format!("{} {}", pattern.title, pattern.error_template),
            tool_name: Some(event.tool_name.clone()),
            context: Some(ContextInfo {
                os: event.os,
                shell: event.shell,
                cli: event.cli,
                model: event.model.clone(),
                repo_hash: event.repo_hash.clone(),
            }),
            limit: 5,
        };
        let ranked = self
            .search_fixes(
                &query,
                query_embedding,
                Some(&signature_hash),
                &event.timestamp,
                &suppressed_fixes,
            )
            .await?;

        let chosen = ranked
            .into_iter()
            .find(|candidate| candidate.score >= self.config.min_inject_confidence);

        let episode = Episode {
            id: event.event_id.clone(),
            session_id: event.session_id.clone(),
            cli: event.cli,
            model: event.model.clone(),
            os: event.os,
            shell: event.shell,
            tool_name: event.tool_name.clone(),
            phase: ToolPhase::PostFailure,
            pattern_id: Some(pattern.id.clone()),
            injected_fix_id: chosen.as_ref().map(|r| r.fix.id.clone()),
            command_scrubbed: event.command_or_input.clone(),
            created_at: event.timestamp.clone(),
        };
        self.store.record_episode(&episode).await?;

        if let Some(ranked_fix) = chosen {
            Ok(IngestOutcome::HintInjected {
                pattern: Box::new(pattern),
                ranked: Box::new(ranked_fix),
            })
        } else {
            Ok(IngestOutcome::RecordedFailure {
                pattern: Box::new(pattern),
            })
        }
    }

    async fn maybe_promote_pattern(
        &self,
        pattern: &mut ErrorPattern,
        event: &ToolEvent,
    ) -> Result<(), StoreError> {
        // Smart cross-repo classification: promote generic errors observed in >= N distinct
        // repositories from `repo` scope to `team` (global) unless tied to `<REPO>` scripts.
        if pattern.scope == Scope::Repo
            && !is_repo_local_artifact(&pattern.error_template, event.command_or_input.as_deref())
        {
            let repo_count = self.store.pattern_repo_count(&pattern.id).await?;
            if repo_count >= self.config.repo_promotion_threshold {
                self.store
                    .update_pattern_scope(&pattern.id, Scope::Team, None, &event.timestamp)
                    .await?;
                pattern.scope = Scope::Team;
                pattern.repo_hash = None;
            }
        }
        Ok(())
    }

    async fn collect_suppressed_and_penalize(
        &self,
        pattern: &ErrorPattern,
        event: &ToolEvent,
    ) -> Result<HashSet<String>, StoreError> {
        let recent = self
            .store
            .recent_session_episodes(&event.session_id, 8)
            .await?;
        let mut suppressed = HashSet::new();
        for (idx, ep) in recent.iter().enumerate() {
            if ep.phase == ToolPhase::PostFailure
                && ep.pattern_id.as_deref() == Some(pattern.id.as_str())
                && let Some(fix_id) = ep.injected_fix_id.as_deref()
            {
                suppressed.insert(fix_id.to_owned());
                // Only penalize the immediately preceding injected hint on immediate repeat failure.
                if idx == 0 {
                    let _ = self
                        .store
                        .record_edge_feedback(
                            &format!("err:{}", pattern.id),
                            EdgeType::ResolvedBy,
                            &format!("fix:{fix_id}"),
                            0,
                            1,
                            0,
                            &event.timestamp,
                        )
                        .await;
                }
            }
        }
        Ok(suppressed)
    }

    async fn handle_ok(&self, event: &ToolEvent) -> Result<IngestOutcome, StoreError> {
        let recent = self
            .store
            .recent_session_episodes(&event.session_id, self.config.recovery_max_turns)
            .await?;

        let episode = Episode {
            id: event.event_id.clone(),
            session_id: event.session_id.clone(),
            cli: event.cli,
            model: event.model.clone(),
            os: event.os,
            shell: event.shell,
            tool_name: event.tool_name.clone(),
            phase: ToolPhase::PostOk,
            pattern_id: None,
            injected_fix_id: None,
            command_scrubbed: event.command_or_input.clone(),
            created_at: event.timestamp.clone(),
        };
        self.store.record_episode(&episode).await?;

        // Find the newest matching failure within the recovery window before another success.
        let matching_failure = recent.iter().find(|ep| {
            ep.phase == ToolPhase::PostFailure
                && ep.pattern_id.is_some()
                && within_window(
                    &ep.created_at,
                    &event.timestamp,
                    self.config.recovery_max_window_secs,
                )
                && intents_overlap(ep, event)
        });

        let Some(failed_ep) = matching_failure else {
            return Ok(IngestOutcome::RecordedOk);
        };
        let Some(pattern_id) = failed_ep.pattern_id.as_deref() else {
            return Ok(IngestOutcome::RecordedOk);
        };

        // Closed-loop positive feedback when a hint was injected on that failure.
        if let Some(injected_fix_id) = failed_ep.injected_fix_id.as_deref() {
            self.store
                .record_edge_feedback(
                    &format!("err:{pattern_id}"),
                    EdgeType::ResolvedBy,
                    &format!("fix:{injected_fix_id}"),
                    1,
                    0,
                    1,
                    &event.timestamp,
                )
                .await?;
            return Ok(IngestOutcome::Recovered {
                pattern_id: pattern_id.to_owned(),
                fix_id: injected_fix_id.to_owned(),
                confirmed_injected: true,
            });
        }

        self.pair_recovery(pattern_id, failed_ep, event).await
    }

    async fn pair_recovery(
        &self,
        pattern_id: &str,
        failed_ep: &Episode,
        event: &ToolEvent,
    ) -> Result<IngestOutcome, StoreError> {
        let before = failed_ep
            .command_scrubbed
            .as_deref()
            .unwrap_or_default()
            .trim();
        let after = event.command_or_input.as_deref().unwrap_or_default().trim();
        if after.is_empty() || before == after {
            return Ok(IngestOutcome::RecordedOk);
        }

        let pattern_scope = self
            .store
            .get_pattern(pattern_id)
            .await?
            .map_or(Scope::Personal, |p| p.scope);

        let fix_sig = compute_signature_hash(pattern_id, after);
        let fix_id = format!("fix-{}", &fix_sig[..16]);
        let summary = build_recovery_summary(event, before, after);
        let fix = Fix {
            id: fix_id.clone(),
            summary,
            fix_type: FixType::Hint,
            pattern_before: (!before.is_empty()).then(|| truncate_chars(before, 240)),
            pattern_after: Some(truncate_chars(after, 240)),
            guard_regex: None,
            status: FixStatus::Candidate,
            scope: pattern_scope,
            source: FixSource::AutoRecovery,
            created_at: event.timestamp.clone(),
            updated_at: event.timestamp.clone(),
        };
        self.store.upsert_fix(&fix).await?;
        self.store
            .upsert_node(&GraphNode {
                id: format!("fix:{fix_id}"),
                kind: NodeKind::Fix,
                label: fix.summary.clone(),
                props_json: json!({"source": "auto_recovery"}),
            })
            .await?;
        self.store
            .record_edge_feedback(
                &format!("err:{pattern_id}"),
                EdgeType::ResolvedBy,
                &format!("fix:{fix_id}"),
                1,
                0,
                1,
                &event.timestamp,
            )
            .await?;

        Ok(IngestOutcome::Recovered {
            pattern_id: pattern_id.to_owned(),
            fix_id,
            confirmed_injected: false,
        })
    }

    async fn record_context_graph(
        &self,
        pattern: &ErrorPattern,
        event: &ToolEvent,
    ) -> Result<(), StoreError> {
        let err_node = format!("err:{}", pattern.id);
        self.store
            .upsert_node(&GraphNode {
                id: err_node.clone(),
                kind: NodeKind::ErrorPattern,
                label: pattern.title.clone(),
                props_json: json!({"tool": pattern.tool_name}),
            })
            .await?;

        let env_id = format!("env:{}:{}", event.os.as_str(), event.shell.as_str());
        let mut context_nodes = vec![
            GraphNode {
                id: env_id,
                kind: NodeKind::Environment,
                label: format!("{}/{}", event.os.as_str(), event.shell.as_str()),
                props_json: json!({"os": event.os.as_str(), "shell": event.shell.as_str()}),
            },
            GraphNode {
                id: format!("cli:{}", event.cli.as_str()),
                kind: NodeKind::Cli,
                label: event.cli.as_str().to_owned(),
                props_json: json!({}),
            },
        ];
        if let Some(model) = event.model.as_deref().filter(|m| !m.is_empty()) {
            context_nodes.push(GraphNode {
                id: format!("model:{model}"),
                kind: NodeKind::Model,
                label: model.to_owned(),
                props_json: json!({}),
            });
        }
        if let Some(repo) = event.repo_hash.as_deref().filter(|r| !r.is_empty()) {
            context_nodes.push(GraphNode {
                id: format!("repo:{repo}"),
                kind: NodeKind::Repo,
                label: repo.to_owned(),
                props_json: json!({}),
            });
        }

        for node in context_nodes {
            let dst = node.id.clone();
            self.store.upsert_node(&node).await?;
            self.store
                .record_edge_feedback(
                    &err_node,
                    EdgeType::OccursIn,
                    &dst,
                    0,
                    0,
                    1,
                    &event.timestamp,
                )
                .await?;
        }
        Ok(())
    }

    /// Hybrid Vector + BM25 + Exact-Signature -> Graph-Expanded ranking with strict repo isolation.
    pub async fn search_fixes(
        &self,
        query: &SearchQuery,
        query_embedding: Option<&[f32]>,
        exact_signature: Option<&str>,
        now_iso: &str,
        exclude_fix_ids: &HashSet<String>,
    ) -> Result<Vec<RankedFix>, StoreError> {
        let mut candidate_ranks: HashMap<String, (bool, Option<usize>, Option<usize>)> =
            HashMap::new();

        if let Some(sig) = exact_signature
            && let Some(pat) = self.store.find_pattern_by_signature(sig).await?
        {
            candidate_ranks.insert(pat.id, (true, None, None));
        }

        for hit in self
            .store
            .search_bm25(&query.text, query.tool_name.as_deref(), 20)
            .await?
        {
            candidate_ranks
                .entry(hit.pattern_id)
                .and_modify(|e| e.1 = Some(hit.rank))
                .or_insert((false, Some(hit.rank), None));
        }

        if let Some(emb) = query_embedding {
            for hit in self.store.search_vector(emb, 20).await? {
                candidate_ranks
                    .entry(hit.pattern_id)
                    .and_modify(|e| e.2 = Some(hit.rank))
                    .or_insert((false, None, Some(hit.rank)));
            }
        }

        let caller_repo = query.context.as_ref().and_then(|c| c.repo_hash.as_deref());
        let caller_cli = query.context.as_ref().map(|c| c.cli.as_str());
        let caller_model = query.context.as_ref().and_then(|c| c.model.as_deref());

        let mut ranked = Vec::new();
        for (pattern_id, (exact, bm25_rank, vec_rank)) in candidate_ranks {
            let Some(pattern) = self.store.get_pattern(&pattern_id).await? else {
                continue;
            };

            // Strict repo isolation: repo-scoped patterns NEVER leak across unrelated repos.
            if pattern.scope == Scope::Repo && pattern.repo_hash.as_deref() != caller_repo {
                continue;
            }

            let relevance = hybrid_relevance(exact, bm25_rank, vec_rank);
            let ctx_nodes = self.store.pattern_contexts(&pattern.id).await?;
            let is_shell = is_shell_tool(&pattern.tool_name);
            let ctx_mult = context_multiplier(query.context.as_ref(), &ctx_nodes, is_shell);

            for candidate in self.store.fixes_for_pattern(&pattern.id).await? {
                if exclude_fix_ids.contains(&candidate.fix.id)
                    || matches!(
                        candidate.fix.status,
                        FixStatus::Deprecated | FixStatus::Obsolete
                    )
                {
                    continue;
                }
                if self
                    .store
                    .is_fix_superseded_or_obsolete(&candidate.fix.id, caller_cli, caller_model)
                    .await?
                {
                    continue;
                }

                let confidence = effective_confidence(
                    candidate.fix.status,
                    candidate.edge.worked_count,
                    candidate.edge.failed_count,
                );
                let decay = recency_decay(
                    &candidate.edge.last_seen_at,
                    now_iso,
                    self.config.recency_half_life_days,
                );
                let score = relevance * ctx_mult * confidence * decay;
                ranked.push(RankedFix {
                    fix: candidate.fix,
                    pattern_id: pattern.id.clone(),
                    score,
                    confidence,
                    worked_count: candidate.edge.worked_count,
                    failed_count: candidate.edge.failed_count,
                });
            }
        }

        ranked.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| b.worked_count.cmp(&a.worked_count))
                .then_with(|| a.fix.id.cmp(&b.fix.id))
        });
        ranked.truncate(query.limit);
        Ok(ranked)
    }
}

fn within_window(earlier_iso: &str, later_iso: &str, max_secs: i64) -> bool {
    let (Some(t1), Some(t2)) = (
        parse_iso_epoch_secs(earlier_iso),
        parse_iso_epoch_secs(later_iso),
    ) else {
        return true;
    };
    let delta = t2 - t1;
    (0..=max_secs).contains(&delta)
}

fn intents_overlap(failed: &Episode, succeeded: &ToolEvent) -> bool {
    if !failed.tool_name.eq_ignore_ascii_case(&succeeded.tool_name) {
        return false;
    }
    let Some(before) = failed.command_scrubbed.as_deref() else {
        return true;
    };
    let Some(after) = succeeded.command_or_input.as_deref() else {
        return true;
    };
    let stem = |cmd: &str| {
        cmd.split_whitespace()
            .next()
            .unwrap_or_default()
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .trim_end_matches(".exe")
            .to_ascii_lowercase()
    };
    let b_stem = stem(before);
    let a_stem = stem(after);
    if !b_stem.is_empty() && b_stem == a_stem {
        return true;
    }
    // Same shell tool with overlapping non-trivial tokens (e.g. target file/script).
    let tokens = |s: &str| -> HashSet<String> {
        s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'))
            .filter(|t| t.len() >= 3)
            .map(str::to_ascii_lowercase)
            .collect()
    };
    let b_toks = tokens(before);
    let a_toks = tokens(after);
    !b_toks.is_disjoint(&a_toks)
}

fn is_shell_tool(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "shell" | "bash" | "sh" | "pwsh" | "powershell" | "cmd" | "run_command" | "exec"
    )
}

fn is_repo_local_artifact(error_template: &str, command: Option<&str>) -> bool {
    let mentions_local = |s: &str| {
        s.contains("<REPO>")
            || s.contains("./scripts/")
            || s.contains(r".\scripts\")
            || s.contains("./gradlew")
            || s.contains(r".\gradlew")
    };
    mentions_local(error_template) || command.is_some_and(mentions_local)
}

fn summarize_title(template: &str) -> String {
    let first_line = template.lines().next().unwrap_or(template).trim();
    truncate_chars(first_line, 120)
}

fn build_recovery_summary(event: &ToolEvent, before: &str, after: &str) -> String {
    let env = format!("{}/{}", event.os.as_str(), event.shell.as_str());
    let single_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let before_line = single_line(before);
    let after_line = single_line(after);
    let summary = if before_line.is_empty() {
        format!("On {env}, use `{}`.", truncate_chars(&after_line, 140))
    } else {
        format!(
            "On {env}, instead of `{}` use `{}`.",
            truncate_chars(&before_line, 70),
            truncate_chars(&after_line, 110)
        )
    };
    truncate_chars(&summary, 240)
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut out: String = input.chars().take(max_chars).collect();
    if input.chars().count() > max_chars && max_chars > 3 {
        out = input.chars().take(max_chars - 3).collect();
        out.push_str("...");
    }
    out
}
