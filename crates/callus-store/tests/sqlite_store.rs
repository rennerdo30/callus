#![allow(clippy::unwrap_used)]

use callus_core::{
    CliKind, EdgeType, EmbeddingStatus, Episode, ErrorPattern, Fix, FixSource, FixStatus, FixType,
    GraphEdge, GraphNode, NodeKind, OsKind, Scope, ShellKind, ToolPhase,
};
use callus_store::{EdgeDirection, KnowledgeStore, SqliteStore, StoreError};
use serde_json::json;
use tempfile::tempdir;

fn sample_pattern(
    id: &str,
    sig: &str,
    title: &str,
    template: &str,
    repo: Option<&str>,
) -> ErrorPattern {
    ErrorPattern {
        id: id.to_owned(),
        signature_hash: sig.to_owned(),
        title: title.to_owned(),
        error_template: template.to_owned(),
        tool_name: "shell".to_owned(),
        scope: if repo.is_some() {
            Scope::Repo
        } else {
            Scope::Team
        },
        repo_hash: repo.map(str::to_owned),
        occurrence_count: 1,
        first_seen_at: "2026-11-27T10:00:00Z".to_owned(),
        last_seen_at: "2026-11-27T10:00:00Z".to_owned(),
        embedding_status: EmbeddingStatus::Pending,
        updated_at: "2026-11-27T10:00:00Z".to_owned(),
    }
}

fn sample_fix(
    id: &str,
    summary: &str,
    fix_type: FixType,
    scope: Scope,
    guard: Option<&str>,
) -> Fix {
    Fix {
        id: id.to_owned(),
        summary: summary.to_owned(),
        fix_type,
        pattern_before: Some("python - << 'EOF'".to_owned()),
        pattern_after: Some("python -c \"...\"".to_owned()),
        guard_regex: guard.map(str::to_owned),
        status: FixStatus::Verified,
        scope,
        source: FixSource::AutoRecovery,
        created_at: "2026-11-27T10:05:00Z".to_owned(),
        updated_at: "2026-11-27T10:05:00Z".to_owned(),
    }
}

#[tokio::test]
async fn wal_mode_idempotent_migrations_and_dim_mismatch_guard() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("callus.db");

    let store = SqliteStore::open(&db_path, 4).unwrap();
    assert_eq!(store.journal_mode().unwrap().to_ascii_lowercase(), "wal");
    assert_eq!(store.embedding_dims(), 4);
    drop(store);

    // Reopening with identical dims succeeds cleanly (idempotent migrations).
    let reopened = SqliteStore::open(&db_path, 4).unwrap();
    assert_eq!(reopened.embedding_dims(), 4);
    drop(reopened);

    // Reopening with mismatched dims MUST fail with DimensionMismatch.
    let err = SqliteStore::open(&db_path, 384).unwrap_err();
    assert!(matches!(
        err,
        StoreError::DimensionMismatch {
            expected: 4,
            actual: 384
        }
    ));
}

#[tokio::test]
async fn pattern_upsert_merges_by_signature_and_bm25_handles_hostile_stderr() {
    let store = SqliteStore::open_in_memory(4).unwrap();

    let p1 = sample_pattern(
        "pat-1",
        "sig-heredoc",
        "PowerShell rejects bash << heredoc operator",
        "ParserError: The '<' operator is reserved for future use.",
        Some("repo-a"),
    );
    let inserted = store.upsert_pattern(&p1).await.unwrap();
    assert_eq!(inserted.id, "pat-1");
    assert_eq!(inserted.occurrence_count, 1);

    // Re-upserting same signature increments occurrence_count and keeps canonical id.
    let mut repeat = sample_pattern(
        "pat-duplicate-id",
        "sig-heredoc",
        "PowerShell rejects bash << heredoc operator",
        "ParserError: The '<' operator is reserved for future use.",
        Some("repo-a"),
    );
    repeat.last_seen_at = "2026-11-27T12:00:00Z".to_owned();
    let merged = store.upsert_pattern(&repeat).await.unwrap();
    assert_eq!(merged.id, "pat-1");
    assert_eq!(merged.occurrence_count, 2);
    assert_eq!(merged.last_seen_at, "2026-11-27T12:00:00Z");

    let p2 = sample_pattern(
        "pat-2",
        "sig-zsh",
        "zsh no matches found unquoted glob",
        "zsh: no matches found: *.rs",
        None,
    );
    store.upsert_pattern(&p2).await.unwrap();

    // Hostile stderr full of FTS5 syntax characters must never error and must rank pat-1 #1.
    let hits = store
        .search_bm25(
            r#"ParserError: The '<' operator is reserved ("future") AND OR NOT * ^ (pwsh)"#,
            Some("shell"),
            10,
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].pattern_id, "pat-1");
    assert_eq!(hits[0].rank, 1);
    assert!(hits[0].score > 0.0);

    // Scope promotion updates scope & repo_hash cleanly.
    store
        .update_pattern_scope("pat-1", Scope::Team, None, "2026-11-27T13:00:00Z")
        .await
        .unwrap();
    let fetched = store.get_pattern("pat-1").await.unwrap().unwrap();
    assert_eq!(fetched.scope, Scope::Team);
    assert_eq!(fetched.repo_hash, None);
}

#[tokio::test]
async fn sqlite_vec_cosine_search_and_dimension_validation() {
    let store = SqliteStore::open_in_memory(4).unwrap();

    // Empty vector table degrades cleanly to empty result.
    assert_eq!(
        store
            .search_vector(&[1.0, 0.0, 0.0, 0.0], 5)
            .await
            .unwrap()
            .len(),
        0
    );

    let p1 = sample_pattern("pat-1", "sig-1", "Heredoc fail", "ParserError", None);
    let p2 = sample_pattern("pat-2", "sig-2", "Glob fail", "no matches found", None);
    store.upsert_pattern(&p1).await.unwrap();
    store.upsert_pattern(&p2).await.unwrap();

    assert_eq!(store.list_pending_embeddings(10).await.unwrap().len(), 2);

    store
        .upsert_pattern_embedding("pat-1", &[1.0, 0.0, 0.0, 0.0])
        .await
        .unwrap();
    store
        .upsert_pattern_embedding("pat-2", &[0.0, 1.0, 0.0, 0.0])
        .await
        .unwrap();

    assert_eq!(store.list_pending_embeddings(10).await.unwrap().len(), 0);
    assert_eq!(
        store
            .get_pattern("pat-1")
            .await
            .unwrap()
            .unwrap()
            .embedding_status,
        EmbeddingStatus::Ready
    );

    let hits = store
        .search_vector(&[0.98, 0.02, 0.0, 0.0], 2)
        .await
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].pattern_id, "pat-1");
    assert_eq!(hits[0].rank, 1);
    assert!(hits[0].distance < hits[1].distance);

    let err = store.search_vector(&[1.0, 0.0], 5).await.unwrap_err();
    assert!(matches!(
        err,
        StoreError::DimensionMismatch {
            expected: 4,
            actual: 2
        }
    ));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn graph_traversal_feedback_repo_isolation_and_obsolescence() {
    let store = SqliteStore::open_in_memory(4).unwrap();

    let pat = sample_pattern(
        "p1",
        "sig-1",
        "Gradle daemon OOM",
        "Metaspace error",
        Some("repo-alpha"),
    );
    store.upsert_pattern(&pat).await.unwrap();

    let repo_guard = sample_fix(
        "f-repo",
        "Pass --no-daemon in repo-alpha",
        FixType::PreToolGuard,
        Scope::Repo,
        Some(r"gradlew\s+test"),
    );
    let global_guard = sample_fix(
        "f-global",
        "Avoid bash heredoc in pwsh",
        FixType::PreToolGuard,
        Scope::Team,
        Some(r"<<\s*'?EOF'?"),
    );
    store.upsert_fix(&repo_guard).await.unwrap();
    store.upsert_fix(&global_guard).await.unwrap();

    for node in [
        GraphNode {
            id: "err:p1".to_owned(),
            kind: NodeKind::ErrorPattern,
            label: "Gradle daemon OOM".to_owned(),
            props_json: json!({}),
        },
        GraphNode {
            id: "fix:f-repo".to_owned(),
            kind: NodeKind::Fix,
            label: "Pass --no-daemon".to_owned(),
            props_json: json!({}),
        },
        GraphNode {
            id: "fix:f-newer".to_owned(),
            kind: NodeKind::Fix,
            label: "Use gradle.properties".to_owned(),
            props_json: json!({}),
        },
        GraphNode {
            id: "repo:repo-alpha".to_owned(),
            kind: NodeKind::Repo,
            label: "repo-alpha".to_owned(),
            props_json: json!({}),
        },
        GraphNode {
            id: "repo:repo-beta".to_owned(),
            kind: NodeKind::Repo,
            label: "repo-beta".to_owned(),
            props_json: json!({}),
        },
        GraphNode {
            id: "env:windows:pwsh".to_owned(),
            kind: NodeKind::Environment,
            label: "windows/pwsh".to_owned(),
            props_json: json!({"os": "windows", "shell": "pwsh"}),
        },
        GraphNode {
            id: "model:future-v9".to_owned(),
            kind: NodeKind::Model,
            label: "future-v9".to_owned(),
            props_json: json!({}),
        },
    ] {
        store.upsert_node(&node).await.unwrap();
    }

    // Link pattern -> fix with atomic feedback deltas.
    let e1 = store
        .record_edge_feedback(
            "err:p1",
            EdgeType::ResolvedBy,
            "fix:f-repo",
            2,
            0,
            2,
            "2026-11-27T11:00:00Z",
        )
        .await
        .unwrap();
    assert_eq!(e1.worked_count, 2);
    let e2 = store
        .record_edge_feedback(
            "err:p1",
            EdgeType::ResolvedBy,
            "fix:f-repo",
            1,
            1,
            2,
            "2026-11-27T12:00:00Z",
        )
        .await
        .unwrap();
    assert_eq!(e2.worked_count, 3);
    assert_eq!(e2.failed_count, 1);

    // Repo-scoped guard fires ONLY in repo-alpha, never in repo-beta or None.
    let alpha_guards = store.list_active_guards(Some("repo-alpha")).await.unwrap();
    assert_eq!(alpha_guards.len(), 2);
    let beta_guards = store.list_active_guards(Some("repo-beta")).await.unwrap();
    assert_eq!(beta_guards.len(), 1);
    assert_eq!(beta_guards[0].id, "f-global");
    let no_repo_guards = store.list_active_guards(None).await.unwrap();
    assert_eq!(no_repo_guards.len(), 1);
    assert_eq!(no_repo_guards[0].id, "f-global");

    // Repo count tracks distinct OCCURS_IN -> repo:* edges.
    store
        .record_edge_feedback(
            "err:p1",
            EdgeType::OccursIn,
            "repo:repo-alpha",
            0,
            0,
            3,
            "2026-11-27T11:00:00Z",
        )
        .await
        .unwrap();
    store
        .record_edge_feedback(
            "err:p1",
            EdgeType::OccursIn,
            "env:windows:pwsh",
            0,
            0,
            3,
            "2026-11-27T11:00:00Z",
        )
        .await
        .unwrap();
    assert_eq!(store.pattern_repo_count("p1").await.unwrap(), 1);
    store
        .record_edge_feedback(
            "err:p1",
            EdgeType::OccursIn,
            "repo:repo-beta",
            0,
            0,
            1,
            "2026-11-27T12:00:00Z",
        )
        .await
        .unwrap();
    assert_eq!(store.pattern_repo_count("p1").await.unwrap(), 2);
    assert_eq!(store.pattern_contexts("p1").await.unwrap().len(), 3);

    // 2-hop neighborhood reaches `fix:f-newer` via `err:p1 -> fix:f-repo -> fix:f-newer`.
    store
        .upsert_edge(&GraphEdge {
            src_id: "fix:f-repo".to_owned(),
            edge_type: EdgeType::ObsoleteIn,
            dst_id: "model:future-v9".to_owned(),
            worked_count: 0,
            failed_count: 0,
            hit_count: 1,
            last_seen_at: "2026-11-27T12:30:00Z".to_owned(),
        })
        .await
        .unwrap();
    assert!(
        !store
            .is_fix_superseded_or_obsolete("f-repo", Some("claude-code"), Some("claude-sonnet-5-5"))
            .await
            .unwrap()
    );
    assert!(
        store
            .is_fix_superseded_or_obsolete("f-repo", Some("claude-code"), Some("future-v9"))
            .await
            .unwrap()
    );

    store
        .upsert_edge(&GraphEdge {
            src_id: "fix:f-repo".to_owned(),
            edge_type: EdgeType::SupersededBy,
            dst_id: "fix:f-newer".to_owned(),
            worked_count: 0,
            failed_count: 0,
            hit_count: 1,
            last_seen_at: "2026-11-27T12:35:00Z".to_owned(),
        })
        .await
        .unwrap();
    assert!(
        store
            .is_fix_superseded_or_obsolete("f-repo", None, None)
            .await
            .unwrap()
    );

    let two_hop = store.neighbors("err:p1", None, 2).await.unwrap();
    assert!(two_hop.iter().any(|n| n.node.id == "fix:f-newer"
        && n.hop == 2
        && n.direction == EdgeDirection::Outgoing));

    let fixes = store.fixes_for_pattern("p1").await.unwrap();
    assert_eq!(fixes.len(), 1);
    assert_eq!(fixes[0].fix.id, "f-repo");
    assert_eq!(fixes[0].edge.worked_count, 3);
}

#[tokio::test]
async fn episodes_pruning_outbox_and_concurrent_wal_access() {
    let dir = tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("concurrent.db"), 4).unwrap();

    let ep_old = Episode {
        id: "ep-1".to_owned(),
        session_id: "sess-1".to_owned(),
        cli: CliKind::ClaudeCode,
        model: Some("gemini-4-neon".to_owned()),
        os: OsKind::Windows,
        shell: ShellKind::Pwsh,
        tool_name: "shell".to_owned(),
        phase: ToolPhase::PostFailure,
        pattern_id: Some("p1".to_owned()),
        injected_fix_id: None,
        command_scrubbed: Some("python - << 'EOF'".to_owned()),
        created_at: "2026-11-10T10:00:00Z".to_owned(),
    };
    let ep_new = Episode {
        id: "ep-2".to_owned(),
        created_at: "2026-11-27T10:00:00Z".to_owned(),
        phase: ToolPhase::PostOk,
        pattern_id: None,
        command_scrubbed: Some("python -c \"print(1)\"".to_owned()),
        ..ep_old.clone()
    };

    let (r1, r2, outbox_id) = tokio::join!(
        store.record_episode(&ep_old),
        store.record_episode(&ep_new),
        store.enqueue_outbox(
            "error_pattern",
            "p1",
            r#"{"delta":1}"#,
            "2026-11-27T10:00:00Z"
        )
    );
    r1.unwrap();
    r2.unwrap();
    assert!(outbox_id.unwrap() > 0);

    let recent = store.recent_session_episodes("sess-1", 5).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].id, "ep-2");
    assert_eq!(recent[1].id, "ep-1");

    let pruned = store
        .prune_episodes_before("2026-11-20T00:00:00Z")
        .await
        .unwrap();
    assert_eq!(pruned, 1);
    assert_eq!(
        store
            .recent_session_episodes("sess-1", 5)
            .await
            .unwrap()
            .len(),
        1
    );

    let outbox = store.list_outbox(10).await.unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].entity_id, "p1");
}
