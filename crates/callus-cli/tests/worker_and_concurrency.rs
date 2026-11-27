#![allow(clippy::unwrap_used)]

use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use callus_core::{
    EdgeType, EmbeddingStatus, ErrorPattern, Fix, FixSource, FixStatus, FixType, GraphNode,
    NodeKind, Scope,
};
use callus_store::{KnowledgeStore, SqliteStore};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener as TokioTcpListener,
};

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

async fn spawn_mock_embedder(dims: usize) -> String {
    let listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = vec![0_u8; 8192];
                let _ = socket.read(&mut buf).await;
                let mut vec = vec![0.0_f32; dims];
                if !vec.is_empty() {
                    vec[0] = 1.0;
                }
                let body = json!({ "data": [{ "index": 0, "embedding": vec }] }).to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{addr}/v1/embeddings")
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn background_consolidation_backfills_embeddings_promotes_repos_and_obsoletes_bad_fixes() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("worker.db");
    let emb_url = spawn_mock_embedder(4).await;

    // Seed 1 pending pattern observed in 2 distinct repos + 1 heavily failing candidate fix.
    {
        let store = SqliteStore::open(&db, 4).unwrap();
        store
            .upsert_pattern(&ErrorPattern {
                id: "pat-w1".to_owned(),
                signature_hash: "sig-w1".to_owned(),
                title: "PowerShell heredoc error".to_owned(),
                error_template: "ParserError: '<' reserved".to_owned(),
                tool_name: "shell".to_owned(),
                scope: Scope::Repo,
                repo_hash: Some("repo-1".to_owned()),
                occurrence_count: 3,
                first_seen_at: "2026-11-27T10:00:00Z".to_owned(),
                last_seen_at: "2026-11-27T10:00:00Z".to_owned(),
                embedding_status: EmbeddingStatus::Pending,
                updated_at: "2026-11-27T10:00:00Z".to_owned(),
            })
            .await
            .unwrap();
        store
            .upsert_fix(&Fix {
                id: "fix-bad".to_owned(),
                summary: "Broken candidate workaround".to_owned(),
                fix_type: FixType::Hint,
                pattern_before: None,
                pattern_after: None,
                guard_regex: None,
                status: FixStatus::Candidate,
                scope: Scope::Repo,
                source: FixSource::AutoRecovery,
                created_at: "2026-11-27T10:00:00Z".to_owned(),
                updated_at: "2026-11-27T10:00:00Z".to_owned(),
            })
            .await
            .unwrap();
        for (id, kind) in [
            ("err:pat-w1", NodeKind::ErrorPattern),
            ("fix:fix-bad", NodeKind::Fix),
            ("repo:repo-1", NodeKind::Repo),
            ("repo:repo-2", NodeKind::Repo),
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
                "err:pat-w1",
                EdgeType::OccursIn,
                "repo:repo-1",
                0,
                0,
                2,
                "2026-11-27T10:00:00Z",
            )
            .await
            .unwrap();
        store
            .record_edge_feedback(
                "err:pat-w1",
                EdgeType::OccursIn,
                "repo:repo-2",
                0,
                0,
                1,
                "2026-11-27T10:01:00Z",
            )
            .await
            .unwrap();
        store
            .record_edge_feedback(
                "err:pat-w1",
                EdgeType::ResolvedBy,
                "fix:fix-bad",
                0,
                4,
                4,
                "2026-11-27T10:02:00Z",
            )
            .await
            .unwrap();
    }

    let port = pick_free_port();
    let bind = format!("127.0.0.1:{port}");
    let base_url = format!("http://{bind}");
    let bin = assert_cmd::cargo::cargo_bin("callus");
    let child = Command::new(bin)
        .args([
            "serve",
            "--bind",
            &bind,
            "--api-token",
            "worker-tok",
            "--worker-interval-secs",
            "0",
            "--db",
        ])
        .arg(&db)
        .env("EMBEDDING_PROVIDER", "openai-compatible")
        .env("EMBEDDING_URL", &emb_url)
        .env("EMBEDDING_DIMS", "4")
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

    // Trigger deterministic consolidation sweep.
    let report: Value = client
        .post(format!("{base_url}/api/v1/admin/consolidate"))
        .bearer_auth("worker-tok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["backfilled_embeddings"], 1);
    assert_eq!(report["promoted_patterns"], 1);
    assert_eq!(report["obsoleted_fixes"], 1);

    // Verify 12 simultaneous users hitting the server concurrently (120 mixed requests).
    let mut handles = Vec::new();
    for user_idx in 0..12_usize {
        let c = client.clone();
        let url = base_url.clone();
        handles.push(tokio::spawn(async move {
            let mut latencies_ms = Vec::new();
            for req_idx in 0..10_usize {
                let start = Instant::now();
                let resp = if (user_idx + req_idx).is_multiple_of(2) {
                    c.get(format!("{url}/api/v1/overview"))
                        .bearer_auth("worker-tok")
                        .send()
                        .await
                        .unwrap()
                } else {
                    c.post(format!("{url}/api/v1/search"))
                        .bearer_auth("worker-tok")
                        .json(&json!({
                            "text": "ParserError heredoc",
                            "tool_name": "shell",
                            "context": null,
                            "limit": 5
                        }))
                        .send()
                        .await
                        .unwrap()
                };
                assert!(resp.status().is_success());
                latencies_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            latencies_ms
        }));
    }

    let mut all_ms = Vec::new();
    for h in handles {
        all_ms.extend(h.await.unwrap());
    }
    all_ms.sort_by(f64::total_cmp);
    assert_eq!(all_ms.len(), 120);
    let p50 = all_ms[all_ms.len() / 2];
    let p95 = all_ms[(all_ms.len() * 95) / 100];
    eprintln!("12-user concurrency bench (120 reqs): p50={p50:.2}ms p95={p95:.2}ms");
    assert!(
        p95 < 500.0,
        "p95 latency too high in debug build: {p95:.2}ms"
    );
}
