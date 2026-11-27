use std::{
    env, fs,
    path::{Path, PathBuf},
};

const FALLBACK_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Callus Web UI</title>
  <style>
    body { font-family: system-ui, sans-serif; background: #0b0e14; color: #f1f5f9; display: grid; place-items: center; min-height: 100vh; margin: 0; }
    main { max-width: 520px; padding: 24px; border: 1px solid #243044; border-radius: 10px; background: #121722; }
    code { background: #192130; padding: 2px 6px; border-radius: 4px; }
  </style>
</head>
<body>
  <main>
    <h1>Callus Web UI bundle missing</h1>
    <p>Build the frontend assets with <code>cd web &amp;&amp; npm ci &amp;&amp; npm run build</code> and rebuild <code>callus</code> to embed the full React 19 UI.</p>
    <p>REST API (<code>/api/v1/overview</code>), health check (<code>/healthz</code>), and Streamable HTTP MCP (<code>/mcp</code>) are active.</p>
  </main>
</body>
</html>
"#;

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-changed=../../web/dist");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let src = manifest_dir.join("../../web/dist");
    let dest = out_dir.join("web-dist");

    if dest.exists() {
        let _ = fs::remove_dir_all(&dest);
    }

    if src.join("index.html").is_file() {
        copy_dir_recursive(&src, &dest).expect("copy web/dist into OUT_DIR/web-dist");
    } else {
        fs::create_dir_all(&dest).expect("create fallback OUT_DIR/web-dist");
        fs::write(dest.join("index.html"), FALLBACK_HTML).expect("write fallback index.html");
    }
}
