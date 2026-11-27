#![allow(clippy::unwrap_used)]

use std::{fmt::Display, str::FromStr};

use callus_core::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

/// `expected` is checked against `as_str`-based `Display`, serde and `FromStr`.
fn check<T>(all: &[T], expected: &[&str])
where
    T: Copy + PartialEq + std::fmt::Debug + Display + FromStr<Err = CallusError>,
    T: Serialize + DeserializeOwned,
{
    assert_eq!(all.len(), expected.len());
    for (&value, &text) in all.iter().zip(expected) {
        assert_eq!(value.to_string(), text);
        assert_eq!(text.parse::<T>().unwrap(), value);
        assert_eq!(serde_json::to_value(value).unwrap(), json!(text));
        assert_eq!(serde_json::from_value::<T>(json!(text)).unwrap(), value);
    }
    assert!(matches!(
        "nope".parse::<T>(),
        Err(CallusError::UnknownVariant { .. })
    ));
}

#[test]
fn enum_strings_match_the_data_model() {
    check(
        CliKind::ALL,
        &["claude-code", "codex", "antigravity", "opencode"],
    );
    check(OsKind::ALL, &["windows", "macos", "linux"]);
    check(
        ShellKind::ALL,
        &["pwsh", "cmd", "bash", "zsh", "fish", "unknown"],
    );
    check(ToolPhase::ALL, &["pre", "post_ok", "post_failure"]);
    check(Scope::ALL, &["personal", "repo", "team", "public-pack"]);
    check(FixType::ALL, &["hint", "pre_tool_guard", "agents_md_rule"]);
    check(
        FixStatus::ALL,
        &["candidate", "verified", "deprecated", "obsolete"],
    );
    check(
        FixSource::ALL,
        &["auto_recovery", "agent_report", "human", "pack"],
    );
    check(EmbeddingStatus::ALL, &["ready", "pending", "skipped"]);
    check(
        NodeKind::ALL,
        &[
            "error_pattern",
            "fix",
            "environment",
            "cli",
            "model",
            "tool",
            "repo",
            "rule",
        ],
    );
    check(
        EdgeType::ALL,
        &[
            "OCCURS_IN",
            "RESOLVED_BY",
            "SUPERSEDED_BY",
            "CAUSES",
            "CONFLICTS_WITH",
            "OBSOLETE_IN",
            "COMPILED_TO",
        ],
    );
    assert_eq!(CliKind::ClaudeCode.as_str(), "claude-code");
}

#[test]
fn tool_event_reads_and_writes_the_documented_shape() {
    let documented = json!({
        "event_id": "01J0000000000000000000000",
        "timestamp": "2026-11-27T13:00:00Z",
        "cli": "claude-code",
        "cli_version": "2.1.x",
        "model": "claude-sonnet-5-5",
        "session_id": "hashed-session-id",
        "os": "windows",
        "shell": "pwsh",
        "repo_hash": "sha256-of-normalized-remote-or-root",
        "tool_name": "bash",
        "phase": "post_failure",
        "command_or_input": "python - << 'EOF'\n...\nEOF",
        "exit_code": 1,
        "error_text": "ParserError: The '<' operator is reserved for future use.",
        "duration_ms": 42
    });
    let event: ToolEvent = serde_json::from_value(documented.clone()).unwrap();
    assert_eq!(event.phase, ToolPhase::PostFailure);
    assert_eq!(serde_json::to_value(&event).unwrap(), documented);
}

#[test]
fn optional_event_fields_may_be_absent() {
    let event: ToolEvent = serde_json::from_value(json!({
        "event_id": "e", "timestamp": "t", "cli": "codex", "session_id": "s",
        "os": "linux", "shell": "bash", "tool_name": "shell", "phase": "pre"
    }))
    .unwrap();
    assert_eq!(event.exit_code, None);
    assert_eq!(event.error_text, None);
}
