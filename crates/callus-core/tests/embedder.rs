#![allow(clippy::unwrap_used)]

use std::{collections::HashMap, time::Duration};

use callus_core::{
    EmbedError, Embedder, EmbeddingConfig, EmbeddingProvider, LocalEmbedder, LocalHardware,
    OpenAiCompatibleEmbedder, build_embedder,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn spawn_mock_server(
    status: &'static str,
    body: String,
    delay: Duration,
    expected_auth: Option<&'static str>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = vec![0_u8; 4096];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let auth_ok = expected_auth.is_none_or(|token| {
                req.to_ascii_lowercase().contains(&format!(
                    "authorization: bearer {}",
                    token.to_ascii_lowercase()
                ))
            });
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let (resp_status, resp_body) = if auth_ok {
                (status, body)
            } else {
                ("401 Unauthorized", r#"{"error":"bad auth"}"#.to_owned())
            };
            let reply = format!(
                "HTTP/1.1 {resp_status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                resp_body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    format!("http://{addr}/v1/embeddings")
}

#[test]
fn config_defaults_to_none_and_redacts_api_key_in_debug() {
    let cfg = EmbeddingConfig::from_lookup(|_| None).unwrap();
    assert_eq!(cfg.provider, EmbeddingProvider::None);
    assert_eq!(cfg.dims, 384);
    assert_eq!(cfg.timeout_ms, 1500);

    let env = HashMap::from([
        ("EMBEDDING_URL", "http://127.0.0.1:8081/v1/embeddings"),
        ("EMBEDDING_MODEL", "bge-m3"),
        ("EMBEDDING_DIMS", "1024"),
        ("EMBEDDING_API_KEY", "sk-super-secret-value"),
        ("EMBEDDING_TIMEOUT_MS", "900"),
    ]);
    let cfg = EmbeddingConfig::from_lookup(|k| env.get(k).map(|v| (*v).to_owned())).unwrap();
    assert_eq!(cfg.provider, EmbeddingProvider::OpenAiCompatible);
    assert_eq!(cfg.dims, 1024);
    assert_eq!(cfg.timeout_ms, 900);
    let debug = format!("{cfg:?}");
    assert!(!debug.contains("sk-super-secret-value"));
    assert!(debug.contains("<redacted>"));
}

#[tokio::test]
async fn noop_embedder_degrades_cleanly() {
    let cfg = EmbeddingConfig::default();
    let embedder = build_embedder(&cfg).unwrap();
    assert!(!embedder.is_enabled());
    assert_eq!(embedder.embed("ParserError").await.unwrap(), None);
    assert_eq!(embedder.health_check().await.unwrap(), None);
}

#[tokio::test]
async fn openai_compatible_embedder_sends_bearer_auth_and_sorts_batch() {
    let body = r#"{"data":[{"index":1,"embedding":[0.0,1.0,0.0,0.0]},{"index":0,"embedding":[1.0,0.0,0.0,0.0]}]}"#.to_owned();
    let url = spawn_mock_server("200 OK", body, Duration::ZERO, Some("my-llama-token")).await;

    let cfg = EmbeddingConfig {
        provider: EmbeddingProvider::OpenAiCompatible,
        url: Some(url),
        model: "bge-small-en-v1.5".to_owned(),
        dims: 4,
        api_key: Some("my-llama-token".to_owned()),
        timeout_ms: 2000,
    };
    let embedder = OpenAiCompatibleEmbedder::new(&cfg).unwrap();
    let vecs = embedder
        .embed_batch_strict(&["first".to_owned(), "second".to_owned()])
        .await
        .unwrap();
    assert_eq!(vecs.len(), 2);
    assert_eq!(vecs[0], vec![1.0, 0.0, 0.0, 0.0]);
    assert_eq!(vecs[1], vec![0.0, 1.0, 0.0, 0.0]);
}

#[tokio::test]
async fn dimension_mismatch_fails_loudly_even_in_soft_embed() {
    let body = r#"{"data":[{"index":0,"embedding":[1.0,2.0]}]}"#.to_owned();
    let url = spawn_mock_server("200 OK", body, Duration::ZERO, None).await;

    let cfg = EmbeddingConfig {
        provider: EmbeddingProvider::OpenAiCompatible,
        url: Some(url),
        model: "bge-small-en-v1.5".to_owned(),
        dims: 4,
        api_key: None,
        timeout_ms: 2000,
    };
    let embedder = OpenAiCompatibleEmbedder::new(&cfg).unwrap();
    let err = embedder.embed("test").await.unwrap_err();
    assert!(matches!(
        err,
        EmbedError::DimensionMismatch {
            expected: 4,
            actual: 2
        }
    ));
}

#[tokio::test]
async fn timeout_and_unreachable_server_degrade_to_none_for_queries() {
    let body = r#"{"data":[{"index":0,"embedding":[1.0,0.0,0.0,0.0]}]}"#.to_owned();
    let slow_url = spawn_mock_server("200 OK", body, Duration::from_millis(400), None).await;

    let slow_cfg = EmbeddingConfig {
        provider: EmbeddingProvider::OpenAiCompatible,
        url: Some(slow_url),
        model: "bge-small-en-v1.5".to_owned(),
        dims: 4,
        api_key: None,
        timeout_ms: 80,
    };
    let slow_embedder = OpenAiCompatibleEmbedder::new(&slow_cfg).unwrap();
    // Soft embed degrades cleanly to None so BM25 + graph keeps working.
    assert_eq!(slow_embedder.embed("timeout test").await.unwrap(), None);

    // Strict call reports Timeout accurately for diagnostics/backfill.
    let slow_url2 = spawn_mock_server(
        "200 OK",
        r#"{"data":[{"index":0,"embedding":[1.0,0.0,0.0,0.0]}]}"#.to_owned(),
        Duration::from_millis(400),
        None,
    )
    .await;
    let strict_embedder = OpenAiCompatibleEmbedder::new(&EmbeddingConfig {
        url: Some(slow_url2),
        ..slow_cfg
    })
    .unwrap();
    let err = strict_embedder
        .embed_batch_strict(&["timeout".to_owned()])
        .await
        .unwrap_err();
    assert!(matches!(err, EmbedError::Timeout(_)));

    // Unreachable port also degrades to None without stalling or erroring.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = listener.local_addr().unwrap();
    drop(listener);
    let dead_embedder = OpenAiCompatibleEmbedder::new(&EmbeddingConfig {
        provider: EmbeddingProvider::OpenAiCompatible,
        url: Some(format!("http://{dead_addr}/v1/embeddings")),
        model: "bge-small-en-v1.5".to_owned(),
        dims: 4,
        api_key: None,
        timeout_ms: 500,
    })
    .unwrap();
    assert_eq!(dead_embedder.embed("offline").await.unwrap(), None);
}

#[test]
fn hardware_gate_distinguishes_dev_machines_from_litevm_containers() {
    // Typical dev laptop/desktop (8 cores, 16 GiB RAM, no tight cgroup).
    let dev = LocalHardware::evaluate(8, 16_384, None);
    assert!(dev.supported);

    // 1-vCPU LiteVM without GPU -> stay on BM25 + Graph unless EMBEDDING_URL is set.
    let lite_cpu = LocalHardware::evaluate(1, 4_096, None);
    assert!(!lite_cpu.supported);

    // Low-RAM host (2 GiB).
    let low_ram = LocalHardware::evaluate(4, 2_048, None);
    assert!(!low_ram.supported);

    // Docker LiteVM with 128 MiB cgroup memory cap on a big host -> stay off local model.
    let lite_container = LocalHardware::evaluate(16, 32_768, Some(128));
    assert!(!lite_container.supported);

    // Container with generous 2 GiB cgroup cap on capable host -> allowed.
    let roomy_container = LocalHardware::evaluate(4, 16_384, Some(2_048));
    assert!(roomy_container.supported);
}

#[test]
fn provider_precedence_prefers_explicit_url_and_none_over_local_auto() {
    // Unset env + capable hardware -> auto-enables built-in local embedder.
    let auto_local = EmbeddingConfig::from_lookup_with_hardware(|_| None, true).unwrap();
    assert_eq!(auto_local.provider, EmbeddingProvider::Local);
    assert_eq!(auto_local.dims, 384);

    // Unset env + constrained hardware -> falls back to None (BM25 + Graph).
    let auto_none = EmbeddingConfig::from_lookup_with_hardware(|_| None, false).unwrap();
    assert_eq!(auto_none.provider, EmbeddingProvider::None);

    // EMBEDDING_URL ALWAYS overrides local auto-detect (for remote GPU server integration).
    let url_env = HashMap::from([("EMBEDDING_URL", "http://gpu-box:8081/v1/embeddings")]);
    let remote = EmbeddingConfig::from_lookup_with_hardware(
        |k| url_env.get(k).map(|v| (*v).to_owned()),
        true,
    )
    .unwrap();
    assert_eq!(remote.provider, EmbeddingProvider::OpenAiCompatible);

    // Explicit EMBEDDING_PROVIDER=none disables local auto-detect.
    let none_env = HashMap::from([("EMBEDDING_PROVIDER", "none")]);
    let disabled = EmbeddingConfig::from_lookup_with_hardware(
        |k| none_env.get(k).map(|v| (*v).to_owned()),
        true,
    )
    .unwrap();
    assert_eq!(disabled.provider, EmbeddingProvider::None);

    // Explicit EMBEDDING_PROVIDER=local forces local even when hardware gate reports false.
    let force_env = HashMap::from([("EMBEDDING_PROVIDER", "local")]);
    let forced = EmbeddingConfig::from_lookup_with_hardware(
        |k| force_env.get(k).map(|v| (*v).to_owned()),
        false,
    )
    .unwrap();
    assert_eq!(forced.provider, EmbeddingProvider::Local);
}

#[tokio::test]
async fn local_embedder_degrades_gracefully_when_offline_and_uncached() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = listener.local_addr().unwrap();
    drop(listener);

    let tmp_dir = std::env::temp_dir().join(format!(
        "callus-local-embedder-offline-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let embedder =
        LocalEmbedder::with_endpoint(384, tmp_dir.clone(), &format!("http://{dead_addr}")).unwrap();
    assert!(embedder.is_enabled());
    assert_eq!(embedder.provider_name(), "local");
    assert_eq!(embedder.dims(), 384);

    // Soft embed returns Ok(None) so BM25 + Graph retrieval continues uninterrupted.
    assert_eq!(embedder.embed("offline test").await.unwrap(), None);
    // Strict batch surfaces a clear network/local error for diagnostics / backfill.
    let err = embedder
        .embed_batch_strict(&["offline test".to_owned()])
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            EmbedError::Http(_) | EmbedError::Timeout(_) | EmbedError::Local(_)
        ),
        "unexpected error variant: {err:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}
