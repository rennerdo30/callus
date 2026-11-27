//! Embedded React 19 SPA static asset serving (`rust-embed`) and `callus ui` local shortcut.

use std::path::PathBuf;

use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use callus_core::CallusError;
use rust_embed::RustEmbed;

use crate::server;

#[derive(RustEmbed)]
#[folder = "$OUT_DIR/web-dist"]
struct WebAssets;

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Local port to bind on 127.0.0.1
    #[arg(long, default_value_t = 8080)]
    port: u16,
    /// Optional path to the local SQLite database
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
}

/// Serves embedded SPA files at `/` and falls back to `index.html` for client-side routes.
pub(crate) async fn serve_embedded(uri: Uri) -> Response {
    let raw_path = uri.path().trim_start_matches('/');
    let asset_path = if raw_path.is_empty() {
        "index.html"
    } else {
        raw_path
    };

    if let Some(file) = WebAssets::get(asset_path) {
        return asset_response(asset_path, file.data.into_owned());
    }

    // SPA fallback for client-side routes (never mask missing API/MCP paths).
    if !asset_path.starts_with("api/")
        && !asset_path.starts_with("mcp")
        && let Some(index) = WebAssets::get("index.html")
    {
        return asset_response("index.html", index.data.into_owned());
    }

    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn asset_response(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, cache)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub(crate) fn run(args: Args) -> Result<(), CallusError> {
    server::run_local_ui(args.port, args.db)
}
