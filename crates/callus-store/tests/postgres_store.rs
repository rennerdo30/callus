#![allow(clippy::unwrap_used)]

use callus_core::{
    CliKind, EdgeType, EmbeddingStatus, ErrorPattern, Fix, FixSource, FixStatus, FixType,
    GraphNode, NodeKind, OsKind, Scope, Scrubber, ScrubberConfig, ShellKind, ToolEvent, ToolPhase,
};
use callus_store::{
    EngineConfig, IngestOutcome, KnowledgeStore, LearningEngine, PgStore, StoreError,
    encode_pgvector,
};
use serde_json::json;

#[test]
fn pgvector_literal_encodes_cleanly() {
    assert_eq!(encode_pgvector(&[1.0, 0.5, -0.25]), "[1,0.5,-0.25]");
    assert_eq!(encode_pgvector(&[]), "[]");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn pg_store_full_hybrid_search_graph_sync_and_learning_e2e() {
    let Ok(db_url) = std::env::var("CALLUS_TEST_DATABASE_URL") else {
        eprintln!("skipping live PostgreSQL test (set CALLUS_TEST_DATABASE_URL to run)");
        return;
    };

    let store = PgStore::connect(&db_url, 4, 4).await.unwrap();
    store.ping().await.unwrap();

    // Clean state for repeatable runs against a shared test DB.
    sqlx::query(
        "TRUNCATE TABLE sync_outbox, episodes, graph_edges, graph_nodes, fixes, error_patterns CASCADE",
    )
    .execute(store.pool())
    .await
    .unwrap();

    // Dimension mismatch guard must fire when reopening with different dims.
    let mismatch = PgStore::connect(&db_url, 384, 2).await.unwrap_err();
    assert!(matches!(
        mismatch,
        StoreError::DimensionMismatch {
            expected: 4,
            actual: 384
        }
    ));

    // Upsert + CRDT occurrence delta merge + pgvector cosine search + tsvector BM25.
    let p1 = ErrorPattern {
        id: "pg-p1".to_owned(),
        signature_hash: "pg-sig-1".to_owned(),
        title: "PowerShell rejects bash heredoc operator".to_owned(),
        error_template: "ParserError: The '<' operator is reserved for future use.".to_owned(),
        tool_name: "shell".to_owned(),
        scope: Scope::Repo,
        repo_hash: Some("repo-pg-a".to_owned()),
        occurrence_count: 1,
        first_seen_at: "2026-11-27T10:00:00Z".to_owned(),
        last_seen_at: "2026-11-27T10:00:00Z".to_owned(),
        embedding_status: EmbeddingStatus::Pending,
        updated_at: "2026-11-27T10:00:00Z".to_owned(),
    };
    store.upsert_pattern(&p1).await.unwrap();
    let merged = store.merge_pattern_delta(&p1, 3).await.unwrap();
    assert_eq!(merged.occurrence_count, 4);

    store
        .upsert_pattern_embedding("pg-p1", &[1.0, 0.0, 0.0, 0.0])
        .await
        .unwrap();
    let vec_hits = store
        .search_vector(&[0.99, 0.01, 0.0, 0.0], 5)
        .await
        .unwrap();
    assert_eq!(vec_hits.len(), 1);
    assert_eq!(vec_hits[0].pattern_id, "pg-p1");

    let bm25_hits = store
        .search_bm25("ParserError operator reserved future", Some("shell"), 5)
        .await
        .unwrap();
    assert_eq!(bm25_hits[0].pattern_id, "pg-p1");

    // LWW fix merge: newer human Verified verdict must NOT be overwritten by older Candidate.
    let verified = Fix {
        id: "pg-f1".to_owned(),
        summary: "Use python -c instead of bash heredoc".to_owned(),
        fix_type: FixType::PreToolGuard,
        pattern_before: Some("python - << 'EOF'".to_owned()),
        pattern_after: Some("python -c \"...\"".to_owned()),
        guard_regex: Some(r"<<\s*'?EOF'?".to_owned()),
        status: FixStatus::Verified,
        scope: Scope::Repo,
        source: FixSource::Human,
        created_at: "2026-11-27T10:00:00Z".to_owned(),
        updated_at: "2026-11-27T12:00:00Z".to_owned(),
    };
    store.merge_fix_lww(&verified).await.unwrap();
    let stale_candidate = Fix {
        status: FixStatus::Candidate,
        updated_at: "2026-11-27T11:00:00Z".to_owned(),
        ..verified.clone()
    };
    let kept = store.merge_fix_lww(&stale_candidate).await.unwrap();
    assert_eq!(kept.status, FixStatus::Verified);

    // Graph link + strict repo isolation on guards.
    for (id, kind) in [
        ("err:pg-p1", NodeKind::ErrorPattern),
        ("fix:pg-f1", NodeKind::Fix),
        ("repo:repo-pg-a", NodeKind::Repo),
    ] {
        store
            .upsert_node(&GraphNode {
                id: id.to_owned(),
                kind,
                label: id.to_owned(),
                props_json: json!({}),
            })
            .await
            .unwrap();
    }
    store
        .record_edge_feedback(
            "err:pg-p1",
            EdgeType::ResolvedBy,
            "fix:pg-f1",
            2,
            0,
            2,
            "2026-11-27T12:00:00Z",
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .list_active_guards(Some("repo-pg-a"))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .list_active_guards(Some("repo-pg-other"))
            .await
            .unwrap()
            .len(),
        0
    );

    // LearningEngine runs identically on PgStore!
    let engine = LearningEngine::new(store, EngineConfig::default());
    let scrubber = Scrubber::new(&ScrubberConfig::default()).unwrap();
    let fail = ToolEvent {
        event_id: "pg-ev-1".to_owned(),
        timestamp: "2026-11-27T13:00:00Z".to_owned(),
        cli: CliKind::ClaudeCode,
        cli_version: None,
        model: None,
        session_id: "pg-sess".to_owned(),
        os: OsKind::Windows,
        shell: ShellKind::Pwsh,
        repo_hash: Some("repo-pg-a".to_owned()),
        tool_name: "shell".to_owned(),
        phase: ToolPhase::PostFailure,
        command_or_input: Some("python - << 'EOF'".to_owned()),
        exit_code: Some(1),
        error_text: Some("ParserError: The '<' operator is reserved for future use.".to_owned()),
        duration_ms: Some(10),
    };
    assert!(matches!(
        engine.ingest_event(&fail, &scrubber, None).await.unwrap(),
        IngestOutcome::HintInjected { .. }
    ));
}
