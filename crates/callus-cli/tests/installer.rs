#![allow(clippy::unwrap_used)]

use std::time::Duration;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::tempdir;

fn callus() -> Command {
    let mut cmd = Command::cargo_bin("callus").unwrap();
    cmd.timeout(Duration::from_secs(30));
    cmd
}

#[test]
#[allow(clippy::too_many_lines)]
fn install_update_uninstall_respects_dry_run_non_tty_safety_and_preserves_user_configs() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let db = home.join("callus.db");

    // Pre-populate a user hook in Claude settings to verify backup + non-destructive merge.
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_settings = claude_dir.join("settings.json");
    std::fs::write(
        &claude_settings,
        r#"{"hooks":{"PreToolUse":[{"matcher":"*","hooks":[{"type":"command","command":"keep-my-hook"}]}]}}"#,
    )
    .unwrap();

    // 1. Unknown tool name exits non-zero cleanly.
    callus()
        .args(["install", "not-a-cli", "--dry-run", "--home"])
        .arg(home)
        .assert()
        .failure();

    // 2. Dry-run shows the exact plan and notes without modifying anything.
    let dry_out = callus()
        .args([
            "install",
            "claude",
            "codex",
            "opencode",
            "antigravity",
            "--dry-run",
            "--no-packs",
            "--home",
        ])
        .arg(home)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let dry_text = String::from_utf8(dry_out).unwrap();
    assert!(dry_text.contains("Dry run: nothing will be changed."));
    assert!(dry_text.contains("claude: these steps will run (install):"));
    assert!(dry_text.contains("codex: these steps will run (install):"));
    assert!(dry_text.contains("/hooks"));
    assert!(!home.join(".codex").join("config.toml").exists());

    // 3. Without `--yes`, non-TTY stdin safely answers "no" and changes nothing.
    let non_tty_out = callus()
        .args(["install", "codex", "--no-packs", "--home"])
        .arg(home)
        .write_stdin("")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let non_tty_text = String::from_utf8(non_tty_out).unwrap();
    assert!(non_tty_text.contains("no terminal to ask on: no"));
    assert!(!home.join(".codex").join("config.toml").exists());

    // 4. `callus install ... --yes --packs` installs all 4 CLIs + starter packs.
    let install_out = callus()
        .args([
            "install",
            "claude",
            "codex",
            "opencode",
            "antigravity",
            "--yes",
            "--packs",
            "--home",
        ])
        .arg(home)
        .arg("--db")
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let install_text = String::from_utf8(install_out).unwrap();
    assert!(install_text.contains("callus ui --port 8080"));
    assert!(home.join(".codex").join("config.toml").exists());
    assert!(
        home.join(".config")
            .join("opencode")
            .join("plugins")
            .join("callus.ts")
            .exists()
    );
    assert!(
        home.join(".gemini")
            .join("config")
            .join("hooks.json")
            .exists()
    );

    // Existing Claude user hook survived alongside Callus hook.
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_settings).unwrap()).unwrap();
    assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);

    // 5. `callus update ... --yes` refreshes cleanly without duplicating entries.
    callus()
        .args(["update", "claude", "codex", "--yes", "--home"])
        .arg(home)
        .assert()
        .success();
    let settings_after_update: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_settings).unwrap()).unwrap();
    assert_eq!(
        settings_after_update["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // 6. `callus uninstall ... --yes` removes only Callus entries and preserves user hook.
    callus()
        .args([
            "uninstall",
            "claude",
            "codex",
            "opencode",
            "antigravity",
            "--yes",
            "--home",
        ])
        .arg(home)
        .assert()
        .success();
    let settings_after_uninstall: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_settings).unwrap()).unwrap();
    let remaining = settings_after_uninstall["hooks"]["PreToolUse"]
        .as_array()
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0]["hooks"][0]["command"], "keep-my-hook");
}
