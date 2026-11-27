#![allow(clippy::unwrap_used)]

use std::{collections::HashMap, time::Duration};

use callus_core::{
    EmbedError, Embedder, EmbeddingConfig, EmbeddingProvider, OpenAiCompatibleEmbedder,
    build_embedder,
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
