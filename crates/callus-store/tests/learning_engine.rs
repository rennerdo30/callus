#![allow(clippy::unwrap_used)]

use std::collections::HashSet;

use callus_core::{
    CliKind, ContextInfo, FixStatus, OsKind, Scope, Scrubber, ScrubberConfig, SearchQuery,
    ShellKind, ToolEvent, ToolPhase,
};
use callus_store::{
    EngineConfig, IngestOutcome, KnowledgeStore, LearningEngine, SqliteStore, recency_decay,
    wilson_lower_bound,
};

#[allow(clippy::too_many_arguments)]
fn shell_event(
    id: &str,
    ts: &str,
    session: &str,
    os: OsKind,
    shell: ShellKind,
    repo: Option<&str>,
    phase: ToolPhase,
    cmd: &str,
    err: Option<&str>,
) -> ToolEvent {
    ToolEvent {
        event_id: id.to_owned(),
        timestamp: ts.to_owned(),
        cli: CliKind::ClaudeCode,
        cli_version: Some("2.1.0".to_owned()),
        model: Some("gemini-4-neon".to_owned()),
        session_id: session.to_owned(),
        os,
        shell,
        repo_hash: repo.map(str::to_owned),
        tool_name: "shell".to_owned(),
        phase,
        command_or_input: Some(cmd.to_owned()),
        exit_code: Some(i32::from(phase == ToolPhase::PostFailure)),
        error_text: err.map(str::to_owned),
        duration_ms: Some(18),
    }
}

#[tokio::test]
async fn powershell_heredoc_end_to_end_learning_injection_and_feedback() {
    let store = SqliteStore::open_in_memory(4).unwrap();
    let engine = LearningEngine::new(store, EngineConfig::default());
    let scrubber = Scrubber::new(&ScrubberConfig::default()).unwrap();

    // 1. Session 1: PowerShell rejects bash heredoc.
    let fail1 = shell_event(
        "ev-1",
        "2026-11-27T10:00:00Z",
        "sess-1",
        OsKind::Windows,
        ShellKind::Pwsh,
        Some("repo-callus"),
        ToolPhase::PostFailure,
        "python - << 'EOF'\nprint(1)\nEOF",
        Some("ParserError: The '<' operator is reserved for future use."),
    );
    let out1 = engine.ingest_event(&fail1, &scrubber, None).await.unwrap();
    assert!(matches!(out1, IngestOutcome::RecordedFailure { .. }));

    // 2. Session 1: Agent recovers 12s later with `python -c`.
    let ok1 = shell_event(
        "ev-2",
        "2026-11-27T10:00:12Z",
        "sess-1",
        OsKind::Windows,
        ShellKind::Pwsh,
        Some("repo-callus"),
        ToolPhase::PostOk,
        r#"python -c "print(1)""#,
        None,
    );
    let out2 = engine.ingest_event(&ok1, &scrubber, None).await.unwrap();
    let (pattern_id, fix_id) = match out2 {
        IngestOutcome::Recovered {
            pattern_id,
            fix_id,
            confirmed_injected,
        } => {
            assert!(!confirmed_injected);
            (pattern_id, fix_id)
        }
        other => panic!("expected Recovered, got {other:?}"),
    };

    // 3. Session 2: Same failure repeats in same repo -> proven fix is injected!
    let fail2 = shell_event(
        "ev-3",
        "2026-11-27T10:10:00Z",
        "sess-2",
        OsKind::Windows,
        ShellKind::Pwsh,
        Some("repo-callus"),
        ToolPhase::PostFailure,
        "python - << 'EOF'\nprint(2)\nEOF",
        Some("ParserError: The '<' operator is reserved for future use."),
    );
    let out3 = engine.ingest_event(&fail2, &scrubber, None).await.unwrap();
    match out3 {
        IngestOutcome::HintInjected { ranked, .. } => {
            assert_eq!(ranked.fix.id, fix_id);
            assert!(ranked.score >= 0.45, "score was {}", ranked.score);
            assert!(ranked.fix.summary.contains("python -c"));
        }
        other => panic!("expected HintInjected, got {other:?}"),
    }

    // 4. Session 2: Agent applies fix and succeeds -> closed-loop positive feedback!
    let ok2 = shell_event(
        "ev-4",
        "2026-11-27T10:10:09Z",
        "sess-2",
        OsKind::Windows,
        ShellKind::Pwsh,
        Some("repo-callus"),
        ToolPhase::PostOk,
        r#"python -c "print(2)""#,
        None,
    );
    let out4 = engine.ingest_event(&ok2, &scrubber, None).await.unwrap();
    assert!(matches!(
        out4,
        IngestOutcome::Recovered {
            confirmed_injected: true,
            ..
        }
    ));

    let fixes = engine.store().fixes_for_pattern(&pattern_id).await.unwrap();
    assert_eq!(fixes[0].edge.worked_count, 2);
    assert_eq!(fixes[0].edge.failed_count, 0);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn strict_repo_isolation_and_automatic_cross_repo_promotion() {
    let store = SqliteStore::open_in_memory(4).unwrap();
    let engine = LearningEngine::new(store, EngineConfig::default());
    let scrubber = Scrubber::new(&ScrubberConfig::default()).unwrap();

    // Learn a pitfall inside repo-a.
    let f_a = shell_event(
        "e1",
        "2026-11-27T10:00:00Z",
        "s1",
        OsKind::Windows,
        ShellKind::Pwsh,
        Some("repo-a"),
        ToolPhase::PostFailure,
        "python - << 'EOF'",
        Some("ParserError: The '<' operator is reserved for future use."),
    );
    engine.ingest_event(&f_a, &scrubber, None).await.unwrap();
    let ok_a = shell_event(
        "e2",
        "2026-11-27T10:00:10Z",
        "s1",
        OsKind::Windows,
        ShellKind::Pwsh,
        Some("repo-a"),
        ToolPhase::PostOk,
        r#"python -c "ok""#,
        None,
    );
    let IngestOutcome::Recovered { pattern_id, .. } =
        engine.ingest_event(&ok_a, &scrubber, None).await.unwrap()
    else {
        panic!("expected recovery");
    };

    // While seen in only 1 repo, pattern stays Scope::Repo and MUST NOT leak to repo-b!
    let pat = engine
        .store()
        .get_pattern(&pattern_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pat.scope, Scope::Repo);
    let query_b = SearchQuery {
        text: "ParserError operator reserved".to_owned(),
        tool_name: Some("shell".to_owned()),
        context: Some(ContextInfo {
            os: OsKind::Windows,
            shell: ShellKind::Pwsh,
            cli: CliKind::ClaudeCode,
            model: None,
            repo_hash: Some("repo-b".to_owned()),
        }),
        limit: 5,
    };
    let leaked = engine
        .search_fixes(
            &query_b,
            None,
            None,
            "2026-11-27T10:05:00Z",
            &HashSet::new(),
        )
        .await
        .unwrap();
    assert_eq!(leaked.len(), 0);

    // Once the SAME generic error also occurs in repo-b (2nd distinct repo), Callus
    // automatically promotes it to Scope::Team and injects the proven fix!
    let f_b = shell_event(
        "e3",
        "2026-11-27T10:06:00Z",
        "s2",
        OsKind::Windows,
        ShellKind::Pwsh,
        Some("repo-b"),
        ToolPhase::PostFailure,
        "python - << 'EOF'",
        Some("ParserError: The '<' operator is reserved for future use."),
    );
    let out_b = engine.ingest_event(&f_b, &scrubber, None).await.unwrap();
    assert!(matches!(out_b, IngestOutcome::HintInjected { .. }));

    let promoted = engine
        .store()
        .get_pattern(&pattern_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(promoted.scope, Scope::Team);
    assert_eq!(promoted.repo_hash, None);

    // Cross-OS query (macOS zsh) must be penalized below injection threshold (0.45).
    let query_mac = SearchQuery {
        text: "ParserError operator reserved".to_owned(),
        tool_name: Some("shell".to_owned()),
        context: Some(ContextInfo {
            os: OsKind::MacOs,
            shell: ShellKind::Zsh,
            cli: CliKind::ClaudeCode,
            model: None,
            repo_hash: Some("repo-c".to_owned()),
        }),
        limit: 5,
    };
    let mac_hits = engine
        .search_fixes(
            &query_mac,
            None,
            None,
            "2026-11-27T10:07:00Z",
            &HashSet::new(),
        )
        .await
        .unwrap();
    assert!(
        mac_hits.iter().all(|h| h.score < 0.45),
        "cross-OS shell fix should stay below 0.45: {mac_hits:?}"
    );
}

#[tokio::test]
async fn negative_feedback_demotes_broken_hint_and_suppresses_injection_loop() {
    let store = SqliteStore::open_in_memory(4).unwrap();
    let engine = LearningEngine::new(store, EngineConfig::default());
    let scrubber = Scrubber::new(&ScrubberConfig::default()).unwrap();

    // Seed a 1/0 candidate fix.
    let f1 = shell_event(
        "f1",
        "2026-11-27T10:00:00Z",
        "s1",
        OsKind::Linux,
        ShellKind::Bash,
        None,
        ToolPhase::PostFailure,
        "cargo test --bad-flag",
        Some("error: unexpected argument '--bad-flag' found"),
    );
    engine.ingest_event(&f1, &scrubber, None).await.unwrap();
    let ok1 = shell_event(
        "o1",
        "2026-11-27T10:00:05Z",
        "s1",
        OsKind::Linux,
        ShellKind::Bash,
        None,
        ToolPhase::PostOk,
        "cargo test --workspace",
        None,
    );
    let IngestOutcome::Recovered {
        pattern_id, fix_id, ..
    } = engine.ingest_event(&ok1, &scrubber, None).await.unwrap()
    else {
        panic!("expected recovery");
    };

    // Session 2: hint is injected on failure.
    let f2 = shell_event(
        "f2",
        "2026-11-27T10:01:00Z",
        "s2",
        OsKind::Linux,
        ShellKind::Bash,
        None,
        ToolPhase::PostFailure,
        "cargo test --bad-flag",
        Some("error: unexpected argument '--bad-flag' found"),
    );
    assert!(matches!(
        engine.ingest_event(&f2, &scrubber, None).await.unwrap(),
        IngestOutcome::HintInjected { .. }
    ));

    // Session 2: immediately fails with the SAME error -> failed_count increments and
    // the same hint is NOT re-injected in a loop!
    let f3 = shell_event(
        "f3",
        "2026-11-27T10:01:08Z",
        "s2",
        OsKind::Linux,
        ShellKind::Bash,
        None,
        ToolPhase::PostFailure,
        "cargo test --bad-flag",
        Some("error: unexpected argument '--bad-flag' found"),
    );
    assert!(matches!(
        engine.ingest_event(&f3, &scrubber, None).await.unwrap(),
        IngestOutcome::RecordedFailure { .. }
    ));

    let fixes = engine.store().fixes_for_pattern(&pattern_id).await.unwrap();
    assert_eq!(fixes[0].fix.id, fix_id);
    assert_eq!(fixes[0].edge.failed_count, 1);
    assert_eq!(fixes[0].fix.status, FixStatus::Candidate);
}

#[test]
fn wilson_and_recency_math_match_spec_invariants() {
    // 25/26 battle-tested fix must clearly outrank a 1/1 lucky hit.
    assert!(wilson_lower_bound(25, 1) > wilson_lower_bound(1, 0) * 3.0);

    // 45-day half-life decay.
    let d0 = recency_decay("2026-10-01T00:00:00Z", "2026-10-01T00:00:00Z", 45.0);
    let d45 = recency_decay("2026-10-01T00:00:00Z", "2026-11-15T00:00:00Z", 45.0);
    assert!((d0 - 1.0).abs() < 1e-6);
    assert!((d45 - 0.5).abs() < 1e-6);
}
