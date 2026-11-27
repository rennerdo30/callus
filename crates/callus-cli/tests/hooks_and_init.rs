#![allow(clippy::unwrap_used)]

use std::time::Duration;

use assert_cmd::Command;
use callus_core::{Fix, FixSource, FixStatus, FixType, Scope};
use callus_store::{DEFAULT_EMBEDDING_DIMS, KnowledgeStore, SqliteStore};
use serde_json::{Value, json};
use tempfile::tempdir;

fn callus() -> Command {
    let mut cmd = Command::cargo_bin("callus").unwrap();
    cmd.timeout(Duration::from_secs(30));
    cmd
}

#[test]
fn claude_hook_learns_recovery_and_injects_additional_context() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("callus.db");

    // 1. Failure in session 1
    let fail1 = json!({
        "session_id": "s-claude-1",
        "tool_name": "Bash",
        "os": "windows",
        "shell": "pwsh",
        "repo_hash": "repo-callus",
        "timestamp": "2026-11-27T10:00:00Z",
        "tool_input": { "command": "python - << 'EOF'\nprint(1)\nEOF" },
        "exit_code": 1,
        "error": "ParserError: The '<' operator is reserved for future use."
    });
    callus()
        .args(["hook", "--cli", "claude", "--event", "failure", "--db"])
        .arg(&db)
        .write_stdin(fail1.to_string())
        .assert()
        .success()
        .stdout("");

    // 2. Recovery in session 1
    let ok1 = json!({
        "session_id": "s-claude-1",
        "tool_name": "Bash",
        "os": "windows",
        "shell": "pwsh",
        "repo_hash": "repo-callus",
        "timestamp": "2026-11-27T10:00:10Z",
        "tool_input": { "command": "python -c \"print(1)\"" },
        "exit_code": 0
    });
    callus()
        .args(["hook", "--cli", "claude", "--event", "post", "--db"])
        .arg(&db)
        .write_stdin(ok1.to_string())
        .assert()
        .success()
        .stdout("");

    // 3. Repeat failure in session 2 -> injects Claude hookSpecificOutput.additionalContext
    let fail2 = json!({
        "session_id": "s-claude-2",
        "tool_name": "Bash",
        "os": "windows",
        "shell": "pwsh",
        "repo_hash": "repo-callus",
        "timestamp": "2026-11-27T10:05:00Z",
        "tool_input": { "command": "python - << 'EOF'\nprint(2)\nEOF" },
        "exit_code": 1,
        "error": "ParserError: The '<' operator is reserved for future use."
    });
    let out = callus()
        .args(["hook", "--cli", "claude", "--event", "failure", "--db"])
        .arg(&db)
        .write_stdin(fail2.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&out).unwrap();
    let ctx = parsed["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(ctx.contains("python -c"), "unexpected hint: {ctx}");
}

#[test]
#[allow(clippy::too_many_lines)]
fn codex_antigravity_and_opencode_adapters_classify_failures_and_guards() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("guards.db");

    // Seed an active team-wide PreToolUse guard against bash heredocs on pwsh.
    let store = SqliteStore::open(&db, DEFAULT_EMBEDDING_DIMS).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(store.upsert_fix(&Fix {
        id: "guard-heredoc".to_owned(),
        summary: "PowerShell rejects `<<` heredocs; use `python -c`.".to_owned(),
        fix_type: FixType::PreToolGuard,
        pattern_before: Some("python - << 'EOF'".to_owned()),
        pattern_after: Some("python -c \"...\"".to_owned()),
        guard_regex: Some(r"<<\s*'?EOF'?".to_owned()),
        status: FixStatus::Verified,
        scope: Scope::Team,
        source: FixSource::Human,
        created_at: "2026-11-27T09:00:00Z".to_owned(),
        updated_at: "2026-11-27T09:00:00Z".to_owned(),
    }))
    .unwrap();
    drop(store);

    // Codex PreToolUse -> {"decision": "block", "reason": "..."}
    let codex_pre = json!({
        "thread_id": "t1",
        "tool_name": "shell",
        "arguments": { "command": "python - << 'EOF'" }
    });
    let out = callus()
        .args(["hook", "--cli", "codex", "--event", "pre", "--db"])
        .arg(&db)
        .write_stdin(codex_pre.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed["decision"], "block");

    // Antigravity camelCase protojson PreToolUse -> {"decision": "deny", "reason": "..."}
    let agy_pre = json!({
        "conversationId": "conv-1",
        "modelName": "gemini-4-neon",
        "toolCall": { "name": "run_command", "args": { "CommandLine": "python - << 'EOF'" } },
        "stepIdx": 3
    });
    let out = callus()
        .args(["hook", "--cli", "antigravity", "--event", "pre", "--db"])
        .arg(&db)
        .write_stdin(agy_pre.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed["decision"], "deny");

    // OpenCode PostToolUse with failure + recovery + repeat failure -> {"hint": "..."}
    let oc_fail = json!({
        "sessionId": "oc-1",
        "tool": "bash",
        "os": "linux",
        "shell": "zsh",
        "timestamp": "2026-11-27T11:00:00Z",
        "args": { "command": "ls *.rs" },
        "output": { "exitCode": 1, "error": "zsh: no matches found: *.rs" }
    });
    callus()
        .args(["hook", "--cli", "opencode", "--event", "post", "--db"])
        .arg(&db)
        .write_stdin(oc_fail.to_string())
        .assert()
        .success();

    let oc_ok = json!({
        "sessionId": "oc-1",
        "tool": "bash",
        "os": "linux",
        "shell": "zsh",
        "timestamp": "2026-11-27T11:00:08Z",
        "args": { "command": "ls '*.rs'" },
        "output": { "exitCode": 0 }
    });
    callus()
        .args(["hook", "--cli", "opencode", "--event", "post", "--db"])
        .arg(&db)
        .write_stdin(oc_ok.to_string())
        .assert()
        .success();

    let oc_fail2 = json!({
        "sessionId": "oc-2",
        "tool": "bash",
        "os": "linux",
        "shell": "zsh",
        "timestamp": "2026-11-27T11:01:00Z",
        "args": { "command": "ls *.rs" },
        "output": { "exitCode": 1, "error": "zsh: no matches found: *.rs" }
    });
    let out = callus()
        .args(["hook", "--cli", "opencode", "--event", "post", "--db"])
        .arg(&db)
        .write_stdin(oc_fail2.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&out).unwrap();
    assert!(parsed["hint"].as_str().unwrap().contains("ls '*.rs'"));

    // Malformed stdin still fails open (exit 0, empty stdout).
    callus()
        .args(["hook", "--cli", "claude", "--event", "failure", "--db"])
        .arg(&db)
        .write_stdin("{not valid json")
        .assert()
        .success()
        .stdout("");
}

#[test]
#[allow(clippy::too_many_lines)]
fn init_is_idempotent_creates_backups_preserves_user_hooks_and_uninstalls() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    // Pre-populate a user hook in Claude settings so we prove non-destructive merge.
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_settings = claude_dir.join("settings.json");
    std::fs::write(
        &claude_settings,
        r#"{"hooks":{"PreToolUse":[{"matcher":"*","hooks":[{"type":"command","command":"my-user-hook"}]}]}}"#,
    )
    .unwrap();

    // 1. Dry-run writes nothing new.
    callus()
        .args([
            "init",
            "--claude",
            "--codex",
            "--antigravity",
            "--opencode",
            "--dry-run",
            "--home",
        ])
        .arg(home)
        .assert()
        .success();
    assert!(!home.join(".codex").join("config.toml").exists());

    // 2. Full install across all 4 CLIs.
    callus()
        .args([
            "init",
            "--claude",
            "--codex",
            "--antigravity",
            "--opencode",
            "--home",
        ])
        .arg(home)
        .assert()
        .success();

    // Verify backup was created for existing Claude settings.json.
    let has_bak = std::fs::read_dir(&claude_dir)
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().contains(".callus-"));
    assert!(
        has_bak,
        "expected .bak backup for pre-existing settings.json"
    );

    // Verify existing user hook survived alongside Callus hook.
    let settings_val: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_settings).unwrap()).unwrap();
    let pre_hooks = settings_val["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre_hooks.len(), 2);

    // Verify Codex TOML, Antigravity hooks/mcp, and OpenCode plugin + config exist.
    let codex_toml = std::fs::read_to_string(home.join(".codex").join("config.toml")).unwrap();
    assert!(codex_toml.contains("hooks = true"));
    assert!(codex_toml.contains("callus hook --cli codex --event pre"));
    assert!(codex_toml.contains("[mcp_servers.callus]"));

    let agy_hooks: Value = serde_json::from_str(
        &std::fs::read_to_string(home.join(".gemini").join("config").join("hooks.json")).unwrap(),
    )
    .unwrap();
    assert!(agy_hooks.get("callus").is_some());

    let oc_plugin = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("callus.ts");
    assert!(oc_plugin.exists());

    // 3. Running init a second time is idempotent (no duplicate entries!).
    callus()
        .args([
            "init",
            "--claude",
            "--codex",
            "--antigravity",
            "--opencode",
            "--home",
        ])
        .arg(home)
        .assert()
        .success();
    let settings_val2: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_settings).unwrap()).unwrap();
    assert_eq!(
        settings_val2["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // 4. Uninstall removes Callus cleanly while preserving the user's hook.
    callus()
        .args([
            "init",
            "--claude",
            "--codex",
            "--antigravity",
            "--opencode",
            "--uninstall",
            "--home",
        ])
        .arg(home)
        .assert()
        .success();
    let settings_after: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_settings).unwrap()).unwrap();
    let remaining = settings_after["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0]["hooks"][0]["command"], "my-user-hook");
    assert!(!oc_plugin.exists());
}
