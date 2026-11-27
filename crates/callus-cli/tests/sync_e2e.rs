#![allow(clippy::unwrap_used)]

use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::Duration,
};

use assert_cmd::Command as AssertCommand;
use callus_core::{
    EdgeType, EmbeddingStatus, ErrorPattern, Fix, FixSource, FixStatus, FixType, GraphNode,
    NodeKind, Scope,
};
use callus_store::{DEFAULT_EMBEDDING_DIMS, KnowledgeStore, SqliteStore};
use serde_json::{Value, json};
use tempfile::tempdir;

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn callus_cmd() -> AssertCommand {
    let mut cmd = AssertCommand::cargo_bin("callus").unwrap();
    cmd.timeout(Duration::from_secs(30));
    cmd
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn two_clients_sync_crdt_counters_and_never_leak_personal_records() {
    let dir = tempdir().unwrap();
    let server_db = dir.path().join("server.db");
    let laptop_a = dir.path().join("laptop_a.db");
    let laptop_b = dir.path().join("laptop_b.db");
    let laptop_other = dir.path().join("laptop_other.db");

    // Seed laptop A with: 1 team pattern (occ=2), 1 repo-shared pattern (occ=1), 1 personal pattern (occ=5).
    {
        let sa = SqliteStore::open(&laptop_a, DEFAULT_EMBEDDING_DIMS).unwrap();
        let team_pat = ErrorPattern {
            id: "pat-team".to_owned(),
            signature_hash: "sig-team-shared".to_owned(),
            title: "PowerShell rejects << heredoc".to_owned(),
            error_template: "ParserError: '<' reserved".to_owned(),
            tool_name: "shell".to_owned(),
            scope: Scope::Team,
            repo_hash: None,
            occurrence_count: 2,
            first_seen_at: "2026-11-27T10:00:00Z".to_owned(),
            last_seen_at: "2026-11-27T10:00:00Z".to_owned(),
            embedding_status: EmbeddingStatus::Pending,
            updated_at: "2026-11-27T10:00:00Z".to_owned(),
        };
        let repo_pat = ErrorPattern {
            id: "pat-repo".to_owned(),
            signature_hash: "sig-repo-shared".to_owned(),
            title: "Project gradle flag".to_owned(),
            error_template: "Metaspace OOM".to_owned(),
            tool_name: "shell".to_owned(),
            scope: Scope::Repo,
            repo_hash: Some("repo-shared".to_owned()),
            occurrence_count: 1,
            first_seen_at: "2026-11-27T10:01:00Z".to_owned(),
            last_seen_at: "2026-11-27T10:01:00Z".to_owned(),
            embedding_status: EmbeddingStatus::Pending,
            updated_at: "2026-11-27T10:01:00Z".to_owned(),
        };
        let personal_pat = ErrorPattern {
            id: "pat-personal".to_owned(),
            signature_hash: "sig-personal-secret".to_owned(),
            title: "My personal alias typo".to_owned(),
            error_template: "personal command not found".to_owned(),
            tool_name: "shell".to_owned(),
            scope: Scope::Personal,
            repo_hash: None,
            occurrence_count: 5,
            first_seen_at: "2026-11-27T10:02:00Z".to_owned(),
            last_seen_at: "2026-11-27T10:02:00Z".to_owned(),
            embedding_status: EmbeddingStatus::Pending,
            updated_at: "2026-11-27T10:02:00Z".to_owned(),
        };
        sa.upsert_pattern(&team_pat).await.unwrap();
        sa.upsert_pattern(&repo_pat).await.unwrap();
        sa.upsert_pattern(&personal_pat).await.unwrap();

        let fix = Fix {
            id: "fix-team".to_owned(),
            summary: "Use python -c instead of heredoc".to_owned(),
            fix_type: FixType::Hint,
            pattern_before: None,
            pattern_after: Some("python -c".to_owned()),
            guard_regex: None,
            status: FixStatus::Verified,
            scope: Scope::Team,
            source: FixSource::Human,
            created_at: "2026-11-27T10:00:00Z".to_owned(),
            updated_at: "2026-11-27T10:05:00Z".to_owned(),
        };
        sa.upsert_fix(&fix).await.unwrap();
        sa.upsert_node(&GraphNode {
            id: "err:pat-team".to_owned(),
            kind: NodeKind::ErrorPattern,
            label: "heredoc".to_owned(),
            props_json: json!({}),
        })
        .await
        .unwrap();
        sa.upsert_node(&GraphNode {
            id: "fix:fix-team".to_owned(),
            kind: NodeKind::Fix,
            label: "python -c".to_owned(),
            props_json: json!({}),
        })
        .await
        .unwrap();
        sa.record_edge_feedback(
            "err:pat-team",
            EdgeType::ResolvedBy,
            "fix:fix-team",
            2,
            0,
            2,
            "2026-11-27T10:05:00Z",
        )
        .await
        .unwrap();
    }

    // Seed laptop B independently with the same team pattern (occ=3, worked=1).
    {
        let sb = SqliteStore::open(&laptop_b, DEFAULT_EMBEDDING_DIMS).unwrap();
        let team_pat_b = ErrorPattern {
            id: "pat-team".to_owned(),
            signature_hash: "sig-team-shared".to_owned(),
            title: "PowerShell rejects << heredoc".to_owned(),
            error_template: "ParserError: '<' reserved".to_owned(),
            tool_name: "shell".to_owned(),
            scope: Scope::Team,
            repo_hash: None,
            occurrence_count: 3,
            first_seen_at: "2026-11-27T10:00:00Z".to_owned(),
            last_seen_at: "2026-11-27T10:10:00Z".to_owned(),
            embedding_status: EmbeddingStatus::Pending,
            updated_at: "2026-11-27T10:10:00Z".to_owned(),
        };
        sb.upsert_pattern(&team_pat_b).await.unwrap();
        sb.upsert_node(&GraphNode {
            id: "err:pat-team".to_owned(),
            kind: NodeKind::ErrorPattern,
            label: "heredoc".to_owned(),
            props_json: json!({}),
        })
        .await
        .unwrap();
        sb.upsert_node(&GraphNode {
            id: "fix:fix-team".to_owned(),
            kind: NodeKind::Fix,
            label: "python -c".to_owned(),
            props_json: json!({}),
        })
        .await
        .unwrap();
        sb.record_edge_feedback(
            "err:pat-team",
            EdgeType::ResolvedBy,
            "fix:fix-team",
            1,
            0,
            1,
            "2026-11-27T10:10:00Z",
        )
        .await
        .unwrap();
    }

    // Start central server.
    let port = pick_free_port();
    let bind = format!("127.0.0.1:{port}");
    let base_url = format!("http://{bind}");
    let bin = assert_cmd::cargo::cargo_bin("callus");
    let child = Command::new(bin)
        .args(["serve", "--bind", &bind, "--api-token", "sync-tok", "--db"])
        .arg(&server_db)
        .env("EMBEDDING_PROVIDER", "none")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _guard = ChildGuard(child);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let mut ready = false;
    for _ in 0..40 {
        if let Ok(r) = client.get(format!("{base_url}/healthz")).send().await
            && r.status().is_success()
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready);

    // Sync laptop A, then laptop B, then laptop_other (different repo).
    for (db, repo) in [
        (&laptop_a, "repo-shared"),
        (&laptop_b, "repo-shared"),
        (&laptop_other, "repo-unrelated"),
    ] {
        callus_cmd()
            .args([
                "sync",
                "--server",
                &base_url,
                "--token",
                "sync-tok",
                "--repo-hash",
                repo,
                "--db",
            ])
            .arg(db)
            .assert()
            .success();
    }

    // Verify server converged CRDT counters (2 + 3 = 5 occurrences; 2 + 1 = 3 worked)
    // and NEVER received `pat-personal`.
    let patterns_resp: Value = client
        .get(format!("{base_url}/api/v1/patterns"))
        .bearer_auth("sync-tok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pats = patterns_resp["patterns"].as_array().unwrap();
    assert_eq!(
        pats.len(),
        2,
        "expected only team + repo patterns: {pats:?}"
    );
    assert!(pats.iter().all(|p| p["id"] != "pat-personal"));
    let team_srv = pats.iter().find(|p| p["id"] == "pat-team").unwrap();
    assert_eq!(team_srv["occurrence_count"], 5);

    let detail: Value = client
        .get(format!("{base_url}/api/v1/patterns/pat-team"))
        .bearer_auth("sync-tok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["fixes"][0]["edge"]["worked_count"], 3);

    // Verify laptop B received `pat-repo` (same repo-shared), while laptop_other did NOT!
    let sb = SqliteStore::open(&laptop_b, DEFAULT_EMBEDDING_DIMS).unwrap();
    assert!(sb.get_pattern("pat-repo").await.unwrap().is_some());
    assert!(sb.get_pattern("pat-personal").await.unwrap().is_none());

    let s_other = SqliteStore::open(&laptop_other, DEFAULT_EMBEDDING_DIMS).unwrap();
    assert!(s_other.get_pattern("pat-team").await.unwrap().is_some());
    assert!(s_other.get_pattern("pat-repo").await.unwrap().is_none());
}
