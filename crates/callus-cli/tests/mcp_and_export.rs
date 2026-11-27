#![allow(clippy::unwrap_used)]

use std::{
    io::{BufRead as _, BufReader, Write},
    process::{Command as StdCommand, Stdio},
    time::Duration,
};

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::tempdir;

fn callus() -> Command {
    let mut cmd = Command::cargo_bin("callus").unwrap();
    cmd.timeout(Duration::from_secs(30));
    cmd
}

fn send_rpc(
    stdin: &mut impl Write,
    reader: &mut BufReader<impl std::io::Read>,
    payload: &Value,
) -> Value {
    writeln!(stdin, "{payload}").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad JSON-RPC reply {line:?}: {e}"))
}

#[test]
#[allow(clippy::too_many_lines)]
fn mcp_stdio_server_exposes_all_four_tools_and_scrubs_reports() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("mcp.db");
    let bin = assert_cmd::cargo::cargo_bin("callus");

    let mut child = StdCommand::new(bin)
        .args(["mcp", "--db"])
        .arg(&db)
        .env("EMBEDDING_PROVIDER", "none")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    // 1. Initialize handshake.
    let init_resp = send_rpc(
        &mut stdin,
        &mut reader,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "callus-test", "version": "0.1" }
            }
        }),
    );
    assert!(
        init_resp.get("result").is_some(),
        "init failed: {init_resp}"
    );

    writeln!(
        stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    )
    .unwrap();
    stdin.flush().unwrap();

    // 2. List tools -> must expose exactly the 4 documented tools.
    let list_resp = send_rpc(
        &mut stdin,
        &mut reader,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    let tools = list_resp["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in [
        "callus_search",
        "callus_report",
        "callus_feedback",
        "callus_export_candidates",
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }

    // 3. Report a pitfall containing a fake secret -> must scrub before storing!
    let report_resp = send_rpc(
        &mut stdin,
        &mut reader,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "callus_report",
                "arguments": {
                    "error_text": "ParserError with token ghp_1234567890abcdef1234567890abcdef1234",
                    "fix_summary": "Use python -c instead of << heredoc on pwsh",
                    "tool_name": "shell",
                    "os": "windows",
                    "shell": "pwsh",
                    "cli": "claude-code",
                    "scope": "team"
                }
            }
        }),
    );
    let report_text = report_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let reported: Value = serde_json::from_str(report_text).unwrap();
    let pattern_id = reported["pattern_id"].as_str().unwrap().to_owned();
    let fix_id = reported["fix_id"].as_str().unwrap().to_owned();

    // 4. Search returns the fix.
    let search_resp = send_rpc(
        &mut stdin,
        &mut reader,
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "callus_search",
                "arguments": {
                    "query": "ParserError",
                    "tool_name": "shell",
                    "os": "windows",
                    "shell": "pwsh",
                    "cli": "claude-code"
                }
            }
        }),
    );
    let search_text = search_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let searched: Value = serde_json::from_str(search_text).unwrap();
    assert_eq!(searched["hits"][0]["fix_id"], fix_id);

    // 5. Feedback increments worked_count.
    let fb_resp = send_rpc(
        &mut stdin,
        &mut reader,
        &json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "callus_feedback",
                "arguments": { "pattern_id": pattern_id, "fix_id": fix_id, "worked": true }
            }
        }),
    );
    let fb_text = fb_resp["result"]["content"][0]["text"].as_str().unwrap();
    let fb: Value = serde_json::from_str(fb_text).unwrap();
    assert_eq!(fb["worked_count"], 2);

    // 6. Export candidates lists our pattern and MUST NOT contain the raw secret.
    let exp_resp = send_rpc(
        &mut stdin,
        &mut reader,
        &json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": { "name": "callus_export_candidates", "arguments": { "scope": "global" } }
        }),
    );
    let exp_text = exp_resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(!exp_text.contains("ghp_1234567890"));
    assert!(exp_text.contains("python -c"));

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}

#[test]
#[allow(clippy::too_many_lines)]
fn export_generates_repo_and_global_agents_md_diffs_hooks_and_packs() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("export.db");

    // Feed a Windows PowerShell heredoc failure + `python -c` recovery via `callus hook`.
    let fail = json!({
        "session_id": "s-exp",
        "tool_name": "Bash",
        "os": "windows",
        "shell": "pwsh",
        "repo_hash": "repo-exp-1",
        "timestamp": "2026-11-27T10:00:00Z",
        "tool_input": { "command": "python - << 'EOF'\nprint(1)\nEOF" },
        "exit_code": 1,
        "error": "ParserError: The '<' operator is reserved for future use."
    });
    callus()
        .args(["hook", "--cli", "claude", "--event", "failure", "--db"])
        .arg(&db)
        .write_stdin(fail.to_string())
        .assert()
        .success();

    let ok = json!({
        "session_id": "s-exp",
        "tool_name": "Bash",
        "os": "windows",
        "shell": "pwsh",
        "repo_hash": "repo-exp-1",
        "timestamp": "2026-11-27T10:00:10Z",
        "tool_input": { "command": "python -c \"print(1)\"" },
        "exit_code": 0
    });
    callus()
        .args(["hook", "--cli", "claude", "--event", "post", "--db"])
        .arg(&db)
        .write_stdin(ok.to_string())
        .assert()
        .success();

    // Repo-scoped export for repo-exp-1 includes the learned rule.
    let out_repo = String::from_utf8(
        callus()
            .args([
                "export",
                "--format",
                "agents-md",
                "--scope",
                "repo",
                "--repo-hash",
                "repo-exp-1",
                "--db",
            ])
            .arg(&db)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(out_repo.contains("+++ b/AGENTS.md"));
    assert!(out_repo.contains("python -c"));

    // Repo-scoped export for an unrelated repo-exp-2 MUST NOT leak repo-exp-1's rule!
    let out_other = String::from_utf8(
        callus()
            .args([
                "export",
                "--format",
                "agents-md",
                "--scope",
                "repo",
                "--repo-hash",
                "repo-exp-2",
                "--db",
            ])
            .arg(&db)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(!out_other.contains("python -c"));

    // Pack and hooks formats emit valid JSON.
    let pack_out = String::from_utf8(
        callus()
            .args([
                "export",
                "--format",
                "pack",
                "--scope",
                "repo",
                "--repo-hash",
                "repo-exp-1",
                "--db",
            ])
            .arg(&db)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    let pack_json: Value = serde_json::from_str(&pack_out).unwrap();
    assert_eq!(pack_json["schema_version"], 1);
    assert_eq!(pack_json["rules"][0]["scope"], "public-pack");
}
