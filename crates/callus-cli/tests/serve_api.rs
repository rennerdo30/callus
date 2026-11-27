#![allow(clippy::unwrap_used)]

use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::Duration,
};

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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn serve_exposes_healthz_bearer_auth_rest_api_and_streamable_http_mcp() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("serve.db");
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
            "secret-team-token",
            "--db",
        ])
        .arg(&db)
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

    // Wait for /healthz (unauthenticated probe).
    let mut ready = false;
    for _ in 0..40 {
        if let Ok(resp) = client.get(format!("{base_url}/healthz")).send().await
            && resp.status().is_success()
        {
            let body: Value = resp.json().await.unwrap();
            assert_eq!(body["status"], "ok");
            assert_eq!(body["backend"], "sqlite");
            assert_eq!(body["auth_enabled"], true);
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "callus serve did not become ready on {base_url}");

    // Embedded SPA HTML is served publicly at `/` without requiring Bearer auth.
    let index_resp = client.get(format!("{base_url}/")).send().await.unwrap();
    assert!(index_resp.status().is_success());
    let content_type = index_resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(content_type.contains("text/html"), "got {content_type}");
    let index_html = index_resp.text().await.unwrap();
    assert!(index_html.contains("Callus"));

    // Protected route rejects missing or wrong Bearer token with 401.
    let unauth = client
        .get(format!("{base_url}/api/v1/overview"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);

    let bad_auth = client
        .get(format!("{base_url}/api/v1/overview"))
        .bearer_auth("wrong-token")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_auth.status(), 401);

    // Valid Bearer token works across REST endpoints.
    let overview: Value = client
        .get(format!("{base_url}/api/v1/overview"))
        .bearer_auth("secret-team-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(overview["pattern_count"], 0);
    assert_eq!(overview["retries_saved"], 0);
    assert!(overview["scope_counts"].is_object());
    assert!(overview["context_breakdown"].is_array());

    // Scrubber playground endpoint redacts secrets cleanly.
    let scrub: Value = client
        .post(format!("{base_url}/api/v1/scrub-test"))
        .bearer_auth("secret-team-token")
        .json(&json!({ "text": "DB_PASS=supersecret 10.0.0.5" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        scrub["scrubbed"],
        "DB_PASS=<REDACTED:ENV_SECRET> <REDACTED:IP>"
    );

    // Streamable HTTP MCP endpoint (`/mcp`) rejects unauthenticated calls...
    let mcp_unauth = client
        .post(format!("{base_url}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1" }
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(mcp_unauth.status(), 401);

    // ...and succeeds with Bearer token!
    let mcp_init = client
        .post(format!("{base_url}/mcp"))
        .bearer_auth("secret-team-token")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1" }
            }
        }))
        .send()
        .await
        .unwrap();
    assert!(
        mcp_init.status().is_success(),
        "expected 2xx from /mcp initialize, got {}",
        mcp_init.status()
    );
    let init_text = mcp_init.text().await.unwrap();
    assert!(
        init_text.contains("protocolVersion"),
        "unexpected /mcp reply: {init_text}"
    );
}
