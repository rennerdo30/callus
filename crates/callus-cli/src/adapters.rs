//! Normalized stdin/stdout hook adapters for the 4 Tier-1 CLIs:
//! Claude Code, `OpenAI` Codex CLI, Antigravity (`agy`), and `OpenCode`.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{SystemTime, UNIX_EPOCH},
};

use callus_core::{CliKind, OsKind, ShellKind, ToolEvent, ToolPhase};
use callus_store::IngestOutcome;
use clap::ValueEnum;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// CLI flag values for `callus hook --cli <CLI>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum AdapterCli {
    Claude,
    Codex,
    Antigravity,
    Opencode,
}

impl AdapterCli {
    pub(crate) const fn kind(self) -> CliKind {
        match self {
            Self::Claude => CliKind::ClaudeCode,
            Self::Codex => CliKind::Codex,
            Self::Antigravity => CliKind::Antigravity,
            Self::Opencode => CliKind::OpenCode,
        }
    }
}

/// Event flag values for `callus hook --event <EVENT>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum HookEventArg {
    Pre,
    Post,
    Failure,
    Stop,
}

/// Normalizes one CLI-specific hook payload into a [`ToolEvent`].
/// Returns `None` for `Stop` events (which trigger session maintenance instead).
pub(crate) fn normalize_hook_payload(
    cli: AdapterCli,
    event: HookEventArg,
    payload: &Value,
) -> Option<ToolEvent> {
    if event == HookEventArg::Stop {
        return None;
    }

    let (
        raw_session,
        model,
        cli_version,
        cwd,
        tool_name,
        cmd,
        exit_code,
        error_text,
        explicit_fail,
    ) = match cli {
        AdapterCli::Claude => extract_claude(payload),
        AdapterCli::Codex => extract_codex(payload),
        AdapterCli::Antigravity => extract_antigravity(payload),
        AdapterCli::Opencode => extract_opencode(payload),
    };

    let phase = match event {
        HookEventArg::Pre => ToolPhase::Pre,
        HookEventArg::Failure => ToolPhase::PostFailure,
        HookEventArg::Post => {
            let failed = explicit_fail
                || exit_code.is_some_and(|code| code != 0)
                || error_text.as_deref().is_some_and(|e| !e.trim().is_empty());
            if failed {
                ToolPhase::PostFailure
            } else {
                ToolPhase::PostOk
            }
        }
        HookEventArg::Stop => return None,
    };

    let timestamp =
        str_field(payload, &["timestamp", "createdAt"]).map_or_else(now_iso_utc, str::to_owned);
    let event_id = str_field(payload, &["event_id", "eventId", "tool_use_id", "callId"])
        .map_or_else(
            || {
                format!(
                    "ev-{}",
                    short_hash(&format!("{timestamp}:{raw_session}:{tool_name}"))
                )
            },
            str::to_owned,
        );
    let session_id = short_hash(&raw_session);
    let os = detect_os(payload);
    let shell = detect_shell(payload, os);
    let cwd_path = cwd
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let repo_hash = str_field(payload, &["repo_hash", "repoHash"])
        .map(str::to_owned)
        .or_else(|| cwd_path.as_deref().and_then(compute_repo_hash));

    Some(ToolEvent {
        event_id,
        timestamp,
        cli: cli.kind(),
        cli_version,
        model,
        session_id,
        os,
        shell,
        repo_hash,
        tool_name: normalize_tool_name(&tool_name),
        phase,
        command_or_input: cmd,
        exit_code,
        error_text,
        duration_ms: payload
            .get("duration_ms")
            .or_else(|| payload.get("durationMs"))
            .and_then(Value::as_u64),
    })
}

static ANSI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("valid ANSI regex"));
static INJECTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(</?\s*(?:system|instructions?|assistant|user|tool_call|developer)\b[^>]*>|ignore\s+(?:all\s+)?(?:previous|prior|above)\s+instructions?|disregard\s+(?:all\s+)?(?:previous|prior)\s+instructions?|system\s+override\s*:?)",
    )
    .expect("valid injection regex")
});

/// Sanitizes any text injected into a coding CLI's `additionalContext` or guard reason:
/// strips ANSI/control/bidi spoofing chars, collapses newlines, neutralizes prompt-override
/// directives, and caps length so injected hints stay strictly data-only.
pub(crate) fn sanitize_injected_hint(raw: &str) -> String {
    let no_ansi = ANSI_RE.replace_all(raw, "");
    let cleaned: String = no_ansi
        .chars()
        .filter(|&c| {
            !matches!(
                c,
                '\u{200B}'..='\u{200F}'
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2066}'..='\u{2069}'
                    | '\u{FEFF}'
            ) && (!c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        })
        .collect();
    let single_line = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let neutralized = INJECTION_RE.replace_all(&single_line, "[redacted-directive]");
    let max_chars = 360;
    if neutralized.chars().count() > max_chars {
        let mut out: String = neutralized.chars().take(max_chars - 1).collect();
        out.push('…');
        out
    } else {
        neutralized.into_owned()
    }
}

/// Formats an [`IngestOutcome`] into the stdout JSON expected by each CLI.
pub(crate) fn format_hook_output(cli: AdapterCli, outcome: &IngestOutcome) -> Option<Value> {
    match outcome {
        IngestOutcome::HintInjected { ranked, .. } => {
            let safe = sanitize_injected_hint(&ranked.fix.summary);
            let note = format!(
                "[Callus fix | confidence {:.0}% | worked {}x] {safe}",
                ranked.confidence * 100.0,
                ranked.worked_count,
            );
            Some(match cli {
                AdapterCli::Claude => json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PostToolUseFailure",
                        "additionalContext": note,
                    }
                }),
                AdapterCli::Codex | AdapterCli::Antigravity => json!({
                    "additionalContext": note,
                }),
                AdapterCli::Opencode => json!({
                    "hint": note,
                }),
            })
        }
        IngestOutcome::GuardTriggered { fix } => {
            let safe = sanitize_injected_hint(&fix.summary);
            let reason = format!("[Callus guard] {safe}");
            Some(match cli {
                AdapterCli::Claude => json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason,
                    }
                }),
                AdapterCli::Codex | AdapterCli::Opencode => json!({
                    "decision": "block",
                    "reason": reason,
                }),
                AdapterCli::Antigravity => json!({
                    "decision": "deny",
                    "reason": reason,
                }),
            })
        }
        IngestOutcome::Allowed
        | IngestOutcome::RecordedFailure { .. }
        | IngestOutcome::Recovered { .. }
        | IngestOutcome::RecordedOk => None,
    }
}

type Extracted = (
    String,         // raw session id
    Option<String>, // model
    Option<String>, // cli_version
    Option<String>, // cwd
    String,         // tool_name
    Option<String>, // command_or_input
    Option<i32>,    // exit_code
    Option<String>, // error_text
    bool,           // explicit failure status
);

fn extract_claude(v: &Value) -> Extracted {
    let session = str_field(v, &["session_id"])
        .unwrap_or("default")
        .to_owned();
    let model = str_field(v, &["model"]).map(str::to_owned);
    let version = str_field(v, &["cli_version", "version"]).map(str::to_owned);
    let cwd = str_field(v, &["cwd"]).map(str::to_owned);
    let tool = str_field(v, &["tool_name", "tool"])
        .unwrap_or("unknown")
        .to_owned();
    let cmd = extract_command(v.get("tool_input").or_else(|| v.get("input")));
    let exit_code = int_field(v, &["exit_code"]).or_else(|| {
        v.get("tool_response")
            .and_then(|r| int_field(r, &["exit_code", "exitCode"]))
    });
    let error = str_field(v, &["error", "error_text"])
        .or_else(|| {
            v.get("tool_response")
                .and_then(|r| str_field(r, &["stderr", "error"]))
        })
        .map(str::to_owned);
    let explicit_fail = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    (
        session,
        model,
        version,
        cwd,
        tool,
        cmd,
        exit_code,
        error,
        explicit_fail,
    )
}

fn extract_codex(v: &Value) -> Extracted {
    let session = str_field(v, &["session_id", "thread_id", "conversation_id"])
        .unwrap_or("default")
        .to_owned();
    let model = str_field(v, &["model"]).map(str::to_owned);
    let version = str_field(v, &["cli_version"]).map(str::to_owned);
    let cwd = str_field(v, &["cwd", "workdir"]).map(str::to_owned);
    let tool = str_field(v, &["tool_name", "tool", "name"])
        .unwrap_or("shell")
        .to_owned();
    let cmd = extract_command(
        v.get("tool_input")
            .or_else(|| v.get("arguments"))
            .or_else(|| v.get("input")),
    );
    let exit_code = int_field(v, &["exit_code", "exitCode"]).or_else(|| {
        v.get("result")
            .and_then(|r| int_field(r, &["exit_code", "exitCode"]))
    });
    let error = str_field(v, &["error", "stderr", "error_text"])
        .or_else(|| {
            v.get("result")
                .and_then(|r| str_field(r, &["stderr", "error"]))
        })
        .map(str::to_owned);
    let explicit_fail = str_field(v, &["status"])
        .is_some_and(|s| s.eq_ignore_ascii_case("error") || s.eq_ignore_ascii_case("failed"));
    (
        session,
        model,
        version,
        cwd,
        tool,
        cmd,
        exit_code,
        error,
        explicit_fail,
    )
}

fn extract_antigravity(v: &Value) -> Extracted {
    let session = str_field(v, &["conversationId", "sessionId", "session_id"])
        .unwrap_or("default")
        .to_owned();
    let model = str_field(v, &["modelName", "model"]).map(str::to_owned);
    let version = str_field(v, &["cliVersion", "version"]).map(str::to_owned);
    let cwd = str_field(v, &["cwd", "workspacePath"]).map(str::to_owned);
    let tool_call = v.get("toolCall");
    let tool = tool_call
        .and_then(|tc| str_field(tc, &["name", "toolName"]))
        .or_else(|| str_field(v, &["toolName", "tool_name"]))
        .unwrap_or("unknown")
        .to_owned();
    let cmd = extract_command(
        tool_call
            .and_then(|tc| tc.get("args").or_else(|| tc.get("input")))
            .or_else(|| v.get("toolArgs")),
    );
    let exit_code = int_field(v, &["exitCode", "exit_code"]).or_else(|| {
        v.get("toolOutput")
            .and_then(|o| int_field(o, &["exitCode", "exit_code"]))
    });
    let error = str_field(v, &["errorMessage", "error", "stderr"])
        .or_else(|| {
            v.get("toolOutput")
                .and_then(|o| str_field(o, &["errorMessage", "stderr", "error"]))
        })
        .map(str::to_owned);
    let explicit_fail = str_field(v, &["status", "stepStatus"])
        .is_some_and(|s| s.eq_ignore_ascii_case("error") || s.eq_ignore_ascii_case("failed"));
    (
        session,
        model,
        version,
        cwd,
        tool,
        cmd,
        exit_code,
        error,
        explicit_fail,
    )
}

fn extract_opencode(v: &Value) -> Extracted {
    let session = str_field(v, &["sessionId", "session_id"])
        .unwrap_or("default")
        .to_owned();
    let model = str_field(v, &["model"]).map(str::to_owned);
    let version = str_field(v, &["version", "cliVersion"]).map(str::to_owned);
    let cwd = str_field(v, &["cwd", "directory"]).map(str::to_owned);
    let tool = str_field(v, &["tool", "toolName", "tool_name"])
        .unwrap_or("unknown")
        .to_owned();
    let cmd = extract_command(v.get("args").or_else(|| v.get("input")));
    let exit_code = int_field(v, &["exitCode", "exit_code"]).or_else(|| {
        v.get("output")
            .and_then(|o| int_field(o, &["exitCode", "exit_code"]))
    });
    let error = str_field(v, &["error", "stderr"])
        .or_else(|| {
            v.get("output")
                .and_then(|o| str_field(o, &["error", "stderr"]))
        })
        .map(str::to_owned);
    let explicit_fail = v.get("failed").and_then(Value::as_bool).unwrap_or(false)
        || str_field(v, &["status"]).is_some_and(|s| s.eq_ignore_ascii_case("error"));
    (
        session,
        model,
        version,
        cwd,
        tool,
        cmd,
        exit_code,
        error,
        explicit_fail,
    )
}

fn str_field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
}

fn int_field(v: &Value, keys: &[&str]) -> Option<i32> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_i64))
        .and_then(|n| i32::try_from(n).ok())
}

fn extract_command(val: Option<&Value>) -> Option<String> {
    let val = val?;
    if let Some(s) = val.as_str() {
        return (!s.is_empty()).then(|| s.to_owned());
    }
    if let Some(obj) = val.as_object() {
        for key in [
            "command",
            "CommandLine",
            "cmd",
            "script",
            "path",
            "file_path",
        ] {
            if let Some(s) = obj.get(key).and_then(Value::as_str)
                && !s.is_empty()
            {
                return Some(s.to_owned());
            }
        }
        return serde_json::to_string(obj).ok();
    }
    None
}

fn normalize_tool_name(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "bash" | "sh" | "pwsh" | "powershell" | "cmd" | "shell" | "run_command" | "exec" => {
            "shell".to_owned()
        }
        other => other.to_owned(),
    }
}

fn detect_os(v: &Value) -> OsKind {
    if let Some(raw) = str_field(v, &["os"])
        && let Ok(parsed) = raw.parse()
    {
        return parsed;
    }
    if cfg!(target_os = "windows") {
        OsKind::Windows
    } else if cfg!(target_os = "macos") {
        OsKind::MacOs
    } else {
        OsKind::Linux
    }
}

fn detect_shell(v: &Value, os: OsKind) -> ShellKind {
    if let Some(raw) = str_field(v, &["shell"])
        && let Ok(parsed) = raw.parse()
    {
        return parsed;
    }
    if let Ok(shell_env) = std::env::var("SHELL") {
        let lower = shell_env.to_ascii_lowercase();
        if lower.contains("zsh") {
            return ShellKind::Zsh;
        }
        if lower.contains("fish") {
            return ShellKind::Fish;
        }
        if lower.contains("bash") {
            return ShellKind::Bash;
        }
    }
    if os == OsKind::Windows {
        if std::env::var_os("PSModulePath").is_some() {
            ShellKind::Pwsh
        } else {
            ShellKind::Cmd
        }
    } else {
        ShellKind::Bash
    }
}

/// Fast pure-Rust repo identity hash: walks up to `.git/config` remote URL or repo root
/// without spawning `git` on the `<5ms` hook hot path.
pub(crate) fn compute_repo_hash(start: &Path) -> Option<String> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let dot_git = dir.join(".git");
        if dot_git.exists() {
            let config_path = if dot_git.is_dir() {
                dot_git.join("config")
            } else {
                // Worktree `.git` file points to `gitdir: <path>`.
                let text = std::fs::read_to_string(&dot_git).ok()?;
                let gitdir = text.trim().strip_prefix("gitdir:").map(str::trim)?;
                let resolved = Path::new(gitdir);
                let full = if resolved.is_absolute() {
                    resolved.to_path_buf()
                } else {
                    dir.join(resolved)
                };
                full.join("config")
            };
            if let Ok(cfg) = std::fs::read_to_string(&config_path)
                && let Some(remote) = extract_origin_url(&cfg)
            {
                return Some(short_hash(&remote));
            }
            let canonical = dir
                .to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase();
            return Some(short_hash(&canonical));
        }
        current = dir.parent();
    }
    None
}

fn extract_origin_url(git_config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in git_config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin = trimmed.eq_ignore_ascii_case("[remote \"origin\"]");
            continue;
        }
        if in_origin
            && let Some((key, value)) = trimmed.split_once('=')
            && key.trim().eq_ignore_ascii_case("url")
        {
            let norm = value.trim().trim_end_matches(".git").to_ascii_lowercase();
            if !norm.is_empty() {
                return Some(norm);
            }
        }
    }
    None
}

fn short_hash(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    digest
        .iter()
        .take(8)
        .fold(String::with_capacity(16), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// Returns current UTC time formatted as `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn now_iso_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    epoch_secs_to_iso(secs)
}

/// Returns UTC time `days` ago formatted as `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn iso_utc_days_ago(days: u64) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        .saturating_sub(days.saturating_mul(86_400));
    epoch_secs_to_iso(secs)
}

fn epoch_secs_to_iso(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    // Howard Hinnant's civil-from-days inverse.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}
