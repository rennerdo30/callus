//! Deterministic error templates and signature hashes.

use std::{borrow::Cow, fmt::Write as _, sync::LazyLock};

use regex::{Captures, Regex};
use sha2::{Digest, Sha256};

/// Ordered `(pattern, replacement)` pairs removing volatile details.
static RULES: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
    [
        // ANSI colour codes
        (r"\x1b\[[0-9;?]*[ -/]*[@-~]", ""),
        // ISO timestamps, dates and clock times
        (r"\b\d{4}-\d{2}-\d{2}(?:[T ]\d{2}:\d{2}(?::\d{2}(?:[.,]\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?", "<TIME>"),
        (r"\b\d{2}:\d{2}:\d{2}(?:[.,]\d+)?\b", "<TIME>"),
        (r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b", "<UUID>"),
        (r"\b[0-7][0-9A-HJKMNP-TV-Z]{25}\b", "<ULID>"),
        // Directories are machine specific; the file name identifies the error.
        (r#"(?:<(?:HOME|REPO)>|\b[A-Za-z]:)(?:[\\/]+[^\\/\s:"'()<>|]+)*[\\/]+([^\\/\s:"'()<>|]+)"#, "<PATH>/${1}"),
        (r#"(^|[\s"'`=(\[])(?:/[^/\s:"'()<>|]+)+/([^/\s:"'()<>|]+)"#, "${1}<PATH>/${2}"),
        // Process ids and ports
        (r"(?i)\b((?:p?pid|tid|process(?: id)?)\s*[:=#]?\s*)\d+\b", "${1}<PID>"),
        (r"(?i)\b(port\s*[:=]?\s*)\d{1,5}\b", "${1}<PORT>"),
        (
            r"(?i)(\blocalhost|\b(?:\d{1,3}\.){3}\d{1,3}|\]|<REDACTED:(?:IP|HOST)>|\.(?:com|net|org|io|local|internal)):\d{2,5}\b",
            "${1}:<PORT>",
        ),
        // Line and column references
        (r"(?i)\b((?:line|col|column|char|character|offset)\s*[:=#]?\s*)\d+", "${1}<N>"),
        (r":\d+:\d+\b", ":<N>:<N>"),
        (r"(<PATH>/[^\s:()]+|\.[A-Za-z][A-Za-z0-9]{0,7}):\d+\b", "${1}:<N>"),
        (r"(\.[A-Za-z][A-Za-z0-9]{0,7})\(\d+(?:,\s*\d+)?\)", "${1}(<N>)"),
        (r"(?m)^\s*\d+\s*\|", "<N> |"),
    ]
    .into_iter()
    .map(|(pattern, replacement)| (compile(pattern), replacement))
    .collect()
});

static HEX: LazyLock<Regex> = LazyLock::new(|| compile(r"\b0[xX][0-9a-fA-F]{6,}\b"));

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).expect("built-in signature pattern must compile")
}

/// Strips volatile details (positions, addresses, ids, times, PIDs, ports,
/// directories) and collapses whitespace so repeats of one error compare equal.
pub fn normalize_error_template(error: &str) -> String {
    let mut text = error.to_owned();
    for (regex, replacement) in RULES.iter() {
        if let Cow::Owned(next) = regex.replace_all(&text, *replacement) {
            text = next;
        }
    }
    let text = HEX.replace_all(&text, |caps: &Captures<'_>| {
        let hex = &caps[0];
        // Windows HRESULT / NTSTATUS values identify the error, so they stay.
        if hex.len() == 10 && matches!(hex.as_bytes()[2], b'8' | b'c' | b'C') {
            hex.to_owned()
        } else {
            "<HEX>".to_owned()
        }
    });
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// SHA-256 (lowercase hex) over the tool name and a normalized template.
pub fn compute_signature_hash(tool_name: &str, normalized_template: &str) -> String {
    let digest = Sha256::new()
        .chain_update(tool_name.as_bytes())
        .chain_update([0])
        .chain_update(normalized_template.as_bytes())
        .finalize();
    digest
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            // Writing into a String cannot fail.
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}
