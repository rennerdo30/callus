#![allow(clippy::unwrap_used)]

use std::time::Duration;

use assert_cmd::Command;
use serde_json::{Value, json};

fn callus() -> Command {
    let mut command = Command::cargo_bin("callus").unwrap();
    command.timeout(Duration::from_secs(30));
    command
}

fn stdout(command: &mut Command) -> String {
    let output = command.assert().success().get_output().stdout.clone();
    String::from_utf8(output).unwrap()
}

#[test]
fn scrubs_stdin_and_keeps_line_structure() {
    let out = stdout(
        callus()
            .arg("scrub-test")
            .write_stdin("ssh 10.0.0.1\nmail a@example.com\n"),
    );
    assert_eq!(out, "ssh <REDACTED:IP>\nmail <REDACTED:EMAIL>\n");
}

#[test]
fn json_lists_categories_and_counts_without_secrets() {
    let out = stdout(
        callus()
            .args(["scrub-test", "--json"])
            .write_stdin("10.0.0.1 10.0.0.2"),
    );
    let value: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        value,
        json!({
            "scrubbed": "<REDACTED:IP> <REDACTED:IP>",
            "redactions": [{ "category": "ip", "count": 2 }]
        })
    );
}

#[test]
fn repo_root_deny_regex_and_hostname_flags_apply() {
    let out = stdout(
        callus()
            .args(["scrub-test", "--repo-root", "/srv/work/widget"])
            .args(["--deny-regex", "TICKET-[0-9]+", "--deny-regex", "falcon"])
            .args(["--hostname", "buildbox", "--hostname", "nas"])
            .write_stdin("/srv/work/widget/a.rs TICKET-42 falcon buildbox nas"),
    );
    assert_eq!(
        out,
        "<REPO>/a.rs <REDACTED:CUSTOM> <REDACTED:CUSTOM> <REDACTED:HOST> <REDACTED:HOST>\n"
    );
}

#[test]
fn positional_text_and_file_input() {
    let out = stdout(callus().args(["scrub-test", "--token abc123"]));
    assert_eq!(out, "--token <REDACTED:ENV_SECRET>\n");

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("scrub-input.txt");
    // UTF-16 with BOM, as Windows PowerShell redirection writes it.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend("at 192.168.0.9".encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(&path, bytes).unwrap();
    let out = stdout(callus().args(["scrub-test", "--file"]).arg(&path));
    assert_eq!(out, "at <REDACTED:IP>\n");
}

#[test]
fn bom_and_invalid_utf8_do_not_hide_secrets() {
    let out = stdout(
        callus()
            .arg("scrub-test")
            .write_stdin(b"\xEF\xBB\xBF\xFF 10.0.0.1".to_vec()),
    );
    assert_eq!(out, "\u{FFFD} <REDACTED:IP>\n");
}

#[test]
fn the_running_users_home_is_scrubbed() {
    let home = std::env::home_dir().unwrap();
    let out = stdout(
        callus()
            .arg("scrub-test")
            .env("HOME", "/opt/people/zed")
            .env("USERPROFILE", r"Q:\people\zed")
            .write_stdin(if cfg!(windows) {
                r"Q:\people\zed\notes"
            } else {
                "/opt/people/zed/notes"
            }),
    );
    assert!(out.starts_with("<HOME>"), "{out} (real home {home:?})");
}

#[test]
fn errors_are_reported_without_panicking() {
    for (args, message) in [
        (
            &["scrub-test", "--deny-regex", "("][..],
            "invalid redaction pattern",
        ),
        (
            &["scrub-test", "--file", "does-not-exist"][..],
            "cannot read does-not-exist",
        ),
    ] {
        let output = callus().args(args).write_stdin("x").assert().code(1);
        let stderr = String::from_utf8_lossy(&output.get_output().stderr).into_owned();
        assert!(stderr.contains(message), "{stderr}");
        assert!(!stderr.contains("panicked"));
    }
    callus()
        .args(["scrub-test", "text", "--file", "x"])
        .assert()
        .code(2);
}

#[test]
fn sync_requires_server_url() {
    let output = callus().arg("sync").assert().code(1);
    let stderr = String::from_utf8_lossy(&output.get_output().stderr).into_owned();
    assert!(stderr.contains("CALLUS_SERVER_URL"), "{stderr}");
}

#[test]
fn hook_fails_open() {
    let db = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("hook-fails-open.db");
    callus()
        .args(["hook", "--cli", "claude", "--event", "failure", "--db"])
        .arg(&db)
        .write_stdin("{}")
        .assert()
        .success()
        .stdout("");
}

#[test]
fn help_lists_every_documented_subcommand() {
    let out = stdout(callus().arg("--help"));
    for command in [
        "init",
        "hook",
        "mcp",
        "serve",
        "sync",
        "export",
        "embed",
        "scrub-test",
    ] {
        assert!(out.contains(command), "{command} missing from:\n{out}");
    }
}

#[test]
fn embed_subcommand_reports_status_without_network() {
    let db = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("embed-status.db");
    let out = stdout(
        callus()
            .env("EMBEDDING_PROVIDER", "none")
            .args(["embed", "--db"])
            .arg(&db),
    );
    assert!(out.contains("Provider:  none"), "{out}");
    assert!(out.contains("Hardware:"), "{out}");
}
