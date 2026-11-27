//! Mandatory client-side privacy scrubber. Runs before any persistence or sync.

use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, Ipv6Addr},
    ops::Range,
    sync::LazyLock,
};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::{CallusError, ToolEvent};

/// User and machine specific redaction settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrubberConfig {
    pub repo_root: Option<String>,
    pub home_dir: Option<String>,
    pub extra_hostnames: Vec<String>,
    /// Applied after the built-in rules, so they cannot weaken them.
    pub deny_regexes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionCategory {
    PrivateKey,
    Secret,
    Auth,
    Jwt,
    EnvSecret,
    HighEntropy,
    RepoPath,
    HomePath,
    Email,
    Ip,
    Host,
    Custom,
}

impl RedactionCategory {
    pub const fn placeholder(self) -> &'static str {
        match self {
            Self::PrivateKey => "<REDACTED:PRIVATE_KEY>",
            Self::Secret => "<REDACTED:SECRET>",
            Self::Auth => "<REDACTED:AUTH>",
            Self::Jwt => "<REDACTED:JWT>",
            Self::EnvSecret => "<REDACTED:ENV_SECRET>",
            Self::HighEntropy => "<REDACTED:HIGH_ENTROPY>",
            Self::RepoPath => "<REPO>",
            Self::HomePath => "<HOME>",
            Self::Email => "<REDACTED:EMAIL>",
            Self::Ip => "<REDACTED:IP>",
            Self::Host => "<REDACTED:HOST>",
            Self::Custom => "<REDACTED:CUSTOM>",
        }
    }
}

/// How often a category fired. Deliberately carries no matched text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionHit {
    pub category: RedactionCategory,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrubResult {
    pub scrubbed: String,
    pub redactions: Vec<RedactionHit>,
}

/// Decides whether the candidate at `range` of `text` really is sensitive.
type Filter = fn(text: &str, range: Range<usize>) -> bool;

#[derive(Debug)]
struct Rule {
    category: RedactionCategory,
    /// The capture group `v` is redacted, or the whole match without one.
    regex: Regex,
    filter: Filter,
}

/// Pre-compiled redaction pipeline; build once, reuse for every text.
#[derive(Debug)]
pub struct Scrubber {
    rules: Vec<Rule>,
}

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<(?:REDACTED:[A-Z_]+|REPO|HOME)>").expect("placeholder pattern must compile")
});
static KEYWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)secret|token|key|passw(?:or)?d|credential|auth|bearer")
        .expect("keyword pattern must compile")
});

const PROVIDER_KEYS: &str = concat!(
    r"(?:AKIA|ASIA)[0-9A-Z]{16,}|gh[pousr]_[A-Za-z0-9_]{16,}|github_pat_[A-Za-z0-9_]{20,}",
    r"|glpat-[A-Za-z0-9_\-]{16,}(?:\.[A-Za-z0-9_\-]+)*",
    // OpenAI (`sk-`, `sk-proj-`) and Anthropic (`sk-ant-`)
    r"|(?-u:\b)sk-[A-Za-z0-9_\-]{16,}",
    r"|AIza[0-9A-Za-z_\-]{30,}|xox[baprs]-[A-Za-z0-9\-]{10,}|(?:sk|rk)_live_[A-Za-z0-9]{10,}",
);

/// A quoted secret value; the unquoted forms end at whitespace or a quote.
const QUOTED: &str = r#""(?:\\.|[^"\\\r\n])*"|'[^'\r\n]*'"#;

/// Where a user name starts: Windows (also WSL / Git Bash), macOS and Linux.
const USER_DIR: &str = r"(?:(?:(?i:[a-z]:|(?:/mnt|/cygdrive)?/[a-z])[\\/]+(?i:users)|\\+(?i:users)|/Users|/home)[\\/]+)";

impl Scrubber {
    pub fn new(config: &ScrubberConfig) -> Result<Self, CallusError> {
        // Order matters: structured secrets first, heuristics and user rules last.
        let mut scrubber = Self { rules: Vec::new() };
        scrubber.add_secret_rules()?;
        scrubber.add_identity_rules(config)?;
        scrubber.add(
            RedactionCategory::HighEntropy,
            r"[A-Za-z0-9+/_\-]{20,}={0,2}",
            is_keyed_high_entropy,
        )?;
        for pattern in &config.deny_regexes {
            scrubber.add(RedactionCategory::Custom, pattern, always)?;
        }
        Ok(scrubber)
    }

    fn add(
        &mut self,
        category: RedactionCategory,
        pattern: &str,
        filter: Filter,
    ) -> Result<(), regex::Error> {
        self.rules.push(Rule {
            category,
            regex: Regex::new(pattern)?,
            filter,
        });
        Ok(())
    }

    fn add_secret_rules(&mut self) -> Result<(), regex::Error> {
        use RedactionCategory as C;
        self.add(
            C::PrivateKey,
            r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY[A-Z ]*-----[\s\S]*?(?:-----END [A-Z0-9 ]*PRIVATE KEY[A-Z ]*-----|\z)",
            always,
        )?;
        self.add(C::Secret, PROVIDER_KEYS, always)?;
        self.add(
            C::Auth,
            r#"(?i)authorization\\*["']?\s*[:=]\s*\\*["']?(?:(?:bearer|basic|token|apikey)\s+)?(?P<v>[^\s"',;<]{6,})"#,
            |text, range| !is_scheme_name(&text[range]),
        )?;
        self.add(
            C::Auth,
            r"(?i)(?-u:\b)(?:bearer|basic)[ \t]+(?P<v>[A-Za-z0-9._~+/\-]{8,}=*)",
            |text, range| !is_plain_word(&text[range]),
        )?;
        self.add(
            C::Auth,
            r#"[A-Za-z][A-Za-z0-9+.\-]*://(?P<v>[^\s/?#"'<>]+)@"#,
            always,
        )?;
        // Passwords with an unescaped `/`, `?` or `#`; `host:8080/@x` is a port.
        self.add(
            C::Auth,
            r#"[A-Za-z][A-Za-z0-9+.\-]*://(?P<v>[^\s/?#"'<>:@]*:[^\s"'<>@]+)@"#,
            |text, range| {
                let password = text[range].split_once(':').map_or("", |(_, rest)| rest);
                let after_digits = password.trim_start_matches(|c: char| c.is_ascii_digit());
                !after_digits.starts_with(['/', '?', '#'])
            },
        )?;
        self.add(
            C::Jwt,
            r"eyJ[A-Za-z0-9_\-]{4,}\.[A-Za-z0-9_\-]{4,}\.[A-Za-z0-9_\-]*(?:\.[A-Za-z0-9_\-]+){0,2}",
            always,
        )?;
        // Command line flags with a space or `=` before the value.
        self.add(
            C::EnvSecret,
            &format!(
                r#"(?i)(?:^|[\s"'])--?(?:token|password|passwd|secret|api[_-]?key|client[_-]?secret|auth[_-]?token|access[_-]?(?:token|key)|private[_-]?key)(?:[ \t]*=[ \t]*|[ \t]+)(?P<v>{QUOTED}|["'`]?[^\s"'`\-][^\s"'`]*)"#
            ),
            always,
        )?;
        // `KEY=value`, `key: value`, `"key": "value"`.
        self.add(
            C::EnvSecret,
            &format!(
                r#"(?i)[A-Za-z0-9_\-]*(?:api[_-]?key|secret|token|passw(?:or)?d|private[_-]?key|access[_-]?key|database[_-]?url)[A-Za-z0-9_\-]*(?:\\*["'])?[ \t]*(?:=>|:=|=|:)[ \t]*(?:\\+["'])?(?P<v>{QUOTED}|["'`]?[^\s"'`]+)"#
            ),
            is_assigned_secret,
        )
    }

    /// Paths, addresses and machine names.
    fn add_identity_rules(&mut self, config: &ScrubberConfig) -> Result<(), regex::Error> {
        use RedactionCategory as C;
        for (category, root) in [
            (C::RepoPath, &config.repo_root),
            (C::HomePath, &config.home_dir),
        ] {
            if let Some(pattern) = root.as_deref().and_then(root_pattern) {
                self.add(category, &pattern, ends_name)?;
            }
        }
        // Any user's home. "Jane Doe" needs capitalised extra words that end at
        // a separator, quote or line end, so that prose after a name survives.
        let spaced = r#"[^\\/\s:*?"<>|]+(?: [\p{Lu}\d(][^\\/\s:*?"'<>|]*){1,3}"#;
        let single = r#"[^\\/\s:*?"<>|]*[^\\/\s:*?"<>|'.,;)\]`]"#;
        self.add(
            C::HomePath,
            &format!(r#"(?P<v>{USER_DIR}{spaced})(?:[\\/"']|(?mR:$))|{USER_DIR}{single}"#),
            always,
        )?;
        self.add(
            C::Email,
            r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}",
            |text, range| !is_asset_or_version(&text[range]),
        )?;
        self.add(C::Ip, r"(?:\d{1,3}\.){3}\d{1,3}", is_private_ipv4)?;
        self.add(
            C::Ip,
            // Ends in a group or `::`, never in the single `:` of a message.
            r"[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){0,6}(?:::|:[0-9A-Fa-f]{1,4})(?:%[A-Za-z0-9_\-]+)?",
            is_private_ipv6,
        )?;
        let hosts: Vec<String> = config
            .extra_hostnames
            .iter()
            .filter(|host| !host.trim().is_empty())
            .map(|host| regex::escape(host.trim()))
            .collect();
        if !hosts.is_empty() {
            let hosts = hosts.join("|");
            self.add(
                C::Host,
                &format!(r"(?i)(?:[a-z0-9_\-]+\.)*(?:{hosts})(?:\.[a-z0-9_\-]+)*"),
                is_bounded,
            )?;
        }
        self.add(
            C::Host,
            r"(?i)(?:[a-z0-9_\-]+\.)+(?:local|internal|corp|lan|home\.arpa)",
            |text, range| {
                // `.env.local` and `threading.local` are code, not machines.
                let name = &text[range.clone()];
                !text[..range.start].ends_with('.')
                    && !name.eq_ignore_ascii_case("threading.local")
                    && is_bounded(text, range)
            },
        )
    }

    /// Redacts `text`. Scrubbing an already scrubbed text changes nothing.
    pub fn scrub_text(&self, text: &str) -> ScrubResult {
        let mut scrubbed = text.to_owned();
        let mut counts = BTreeMap::new();
        // A redaction can expose a new boundary to an earlier rule, so repeat
        // until stable. Every change removes unredacted text.
        loop {
            let mut changed = false;
            for rule in &self.rules {
                if let Some((next, count)) = rule.apply(&scrubbed) {
                    scrubbed = next;
                    *counts.entry(rule.category).or_insert(0) += count;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let redactions = counts
            .into_iter()
            .map(|(category, count)| RedactionHit { category, count })
            .collect();
        ScrubResult {
            scrubbed,
            redactions,
        }
    }

    /// Scrubs every free-form field of an event in place.
    pub fn scrub_event(&self, event: &mut ToolEvent) -> Vec<RedactionHit> {
        let mut counts = BTreeMap::new();
        let fields = [
            Some(&mut event.tool_name),
            event.cli_version.as_mut(),
            event.model.as_mut(),
            event.command_or_input.as_mut(),
            event.error_text.as_mut(),
        ];
        for field in fields.into_iter().flatten() {
            let result = self.scrub_text(field);
            *field = result.scrubbed;
            for hit in result.redactions {
                *counts.entry(hit.category).or_insert(0) += hit.count;
            }
        }
        counts
            .into_iter()
            .map(|(category, count)| RedactionHit { category, count })
            .collect()
    }
}

impl Rule {
    /// Returns the new text and number of redactions, or `None` if unchanged.
    fn apply(&self, text: &str) -> Option<(String, usize)> {
        let mut output = String::new();
        let (mut copied, mut count) = (0, 0);
        let hidden: Vec<_> = PLACEHOLDER.find_iter(text).map(|m| m.range()).collect();
        for captures in self.regex.captures_iter(text) {
            let range = match (captures.name("v"), captures.get(0)) {
                (Some(value), _) => unquoted(text, value.range()),
                (None, Some(whole)) => whole.range(),
                (None, None) => continue,
            };
            if range.is_empty()
                || is_redacted(&hidden, &range)
                || !(self.filter)(text, range.clone())
            {
                continue;
            }
            output.push_str(&text[copied..range.start]);
            output.push_str(self.category.placeholder());
            copied = range.end;
            count += 1;
        }
        (count > 0).then(|| {
            output.push_str(&text[copied..]);
            (output, count)
        })
    }
}

/// Narrows a quoted value to its content.
fn unquoted(text: &str, range: Range<usize>) -> Range<usize> {
    let value = &text[range.clone()];
    let Some(quote @ ('"' | '\'' | '`')) = value.chars().next() else {
        // Keep the `\` of a closing `\"` in JSON-escaped text.
        if text[range.end..].starts_with('"') {
            return range.start..range.start + value.trim_end_matches('\\').len();
        }
        return range;
    };
    let end = usize::from(value.len() > 1 && value.ends_with(quote));
    range.start + 1..range.end - end
}

/// True when the range holds only placeholders (`hidden`) or cuts into one.
fn is_redacted(hidden: &[Range<usize>], range: &Range<usize>) -> bool {
    let mut covered = 0;
    let first = hidden.partition_point(|h| h.end <= range.start);
    for found in hidden[first..].iter().take_while(|h| h.start < range.end) {
        if found.start < range.start || found.end > range.end {
            return true;
        }
        covered += found.len();
    }
    covered == range.len()
}

fn always(_: &str, _: Range<usize>) -> bool {
    true
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// The match does not stop inside a longer name (`app` in `app-data`).
fn ends_name(text: &str, range: Range<usize>) -> bool {
    let mut after = text[range.end..].chars();
    !match after.next() {
        Some('.') => after.next().is_some_and(is_name_char),
        next => next.is_some_and(is_name_char),
    }
}

/// The match is not just a part of a longer name.
fn is_bounded(text: &str, range: Range<usize>) -> bool {
    !text[..range.start].ends_with(is_name_char) && ends_name(text, range)
}

/// A pattern matching a configured directory with either separator.
fn root_pattern(root: &str) -> Option<String> {
    let mut parts = root.split(['/', '\\']).filter(|part| !part.is_empty());
    let first = parts.next()?;
    let rest: Vec<String> = parts.map(regex::escape).collect();
    let drive = first.strip_suffix(':').filter(|letter| letter.len() == 1);
    let head = match drive {
        // `C:\x` is also spelled `/c/x` and `/mnt/c/x`.
        Some(letter) => format!(r"(?i)(?:{letter}:|(?:/mnt|/cygdrive)?/{letter})"),
        None if root.starts_with(['/', '\\']) => format!(r"[\\/]+{}", regex::escape(first)),
        None => regex::escape(first),
    };
    // A bare drive or filesystem root would redact every path.
    if rest.is_empty() && (drive.is_some() || !root.starts_with('/')) {
        return None;
    }
    Some([vec![head], rest].concat().join(r"[\\/]+"))
}

fn is_scheme_name(value: &str) -> bool {
    ["bearer", "apikey", "digest", "negotiate"]
        .iter()
        .any(|s| value.eq_ignore_ascii_case(s))
}

/// "Basic authentication" is prose, not a credential.
fn is_plain_word(value: &str) -> bool {
    value
        .chars()
        .skip(1)
        .all(|c| c.is_ascii_lowercase() || c == '-' || c == '.')
}

/// Separates assignments from prose such as `Unexpected token: '}'`.
fn is_assigned_secret(text: &str, range: Range<usize>) -> bool {
    let value = &text[range.clone()];
    let line = text[..range.start].rsplit('\n').next().unwrap_or_default();
    let line = line.trim_end_matches(['"', '\'', '`', '\\', ' ', '\t']);
    // `a == b` and `Token::new` are code.
    if value.starts_with(['=', ':']) {
        return false;
    }
    let assigns = line.ends_with(['=', '>']);
    let key = line.trim_end_matches([':', '=', ' ', '\t']);
    let quoted_key = key.ends_with(['"', '\'']);
    let name = key.trim_end_matches(['"', '\'', '\\']);
    let name = name.rsplit(|c| !is_name_char(c)).next().unwrap_or_default();
    let env_name = !name.is_empty() && !name.chars().any(char::is_lowercase);
    // `max_tokens=4096` is a setting.
    let small_number = value.len() < 8
        && value.bytes().all(|b| b.is_ascii_digit())
        && name.to_ascii_lowercase().ends_with("tokens");
    let compound = name.contains(['_', '-']);
    // A quoted literal after `apiKey:` or after a key that starts the line.
    let literal = text[..range.start].ends_with(['"', '\'', '`'])
        && (compound
            || name.chars().skip(1).any(char::is_uppercase)
            || key.trim_start_matches([' ', '\t', '{', ',', '"', '\'']) == name);
    let special = value
        .chars()
        .any(|c| c.is_ascii_digit() || "!@#$%^*+=".contains(c));
    let secretive = (value.len() >= 6 && (special || literal)) || (compound && value.len() >= 8);
    !small_number && (assigns || quoted_key || env_name || secretive)
}

/// `pkg@1.0.0-rc` and `logo@2x.png` are not email addresses.
fn is_asset_or_version(address: &str) -> bool {
    let domain = address.rsplit('@').next().unwrap_or_default();
    let label = domain.split('.').next().unwrap_or_default();
    let digits = label.strip_suffix('x').unwrap_or(label);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// RFC1918, link-local and CGNAT addresses. Loopback and public ones stay.
fn is_private_ipv4(text: &str, range: Range<usize>) -> bool {
    let digit = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit());
    let mut before = text[..range.start].chars().rev();
    let mut after = text[range.end..].chars();
    let (b, a) = (before.next(), after.next());
    // Part of a longer dotted number, e.g. a version string.
    if digit(b)
        || digit(a)
        || (b == Some('.') && digit(before.next()))
        || (a == Some('.') && digit(after.next()))
    {
        return false;
    }
    text[range].parse::<Ipv4Addr>().is_ok_and(|ip| {
        let [first, second, ..] = ip.octets();
        ip.is_private() || ip.is_link_local() || (first == 100 && (64..128).contains(&second))
    })
}

/// Unique-local (`fc00::/7`) and link-local (`fe80::/10`) addresses.
fn is_private_ipv6(text: &str, range: Range<usize>) -> bool {
    let value = text[range].split('%').next().unwrap_or_default();
    // The candidate may have picked up a preceding `:` of the message.
    [value, value.strip_prefix(':').unwrap_or(value)]
        .iter()
        .find_map(|v| v.parse::<Ipv6Addr>().ok())
        .is_some_and(|ip| ip.is_unique_local() || ip.is_unicast_link_local())
}

/// Shannon entropy in bits per byte.
fn entropy(value: &str) -> f64 {
    let mut seen = [0_u32; 256];
    for byte in value.bytes() {
        seen[usize::from(byte)] += 1;
    }
    let length = f64::from(u32::try_from(value.len()).unwrap_or(u32::MAX));
    seen.iter()
        .filter(|&&n| n > 0)
        .map(|&n| f64::from(n) / length)
        .map(|p| -p * p.log2())
        .sum()
}

fn is_keyed_high_entropy(text: &str, range: Range<usize>) -> bool {
    let value = &text[range.clone()];
    // Identifiers such as `InvalidAuthenticationException` have no digits and
    // change case far less often than random strings.
    let switches = value
        .as_bytes()
        .windows(2)
        .filter(|w| {
            w[0].is_ascii_lowercase() != w[1].is_ascii_lowercase()
                && w[0].is_ascii_alphabetic()
                && w[1].is_ascii_alphabetic()
        })
        .count();
    let identifier = !value.bytes().any(|b| b.is_ascii_digit()) && switches * 3 < value.len();
    if identifier || entropy(value) < 3.8 {
        return false;
    }
    // A keyword shortly before or after on the same line. Placeholder names
    // (`<REDACTED:SECRET>`) do not count. The slices bound the work per match.
    let from = text.floor_char_boundary(range.start.saturating_sub(200));
    let to = text.floor_char_boundary(range.end.saturating_add(200));
    let before = text[from..range.start]
        .rsplit('\n')
        .next()
        .unwrap_or_default();
    let after = text[range.end..to].split('\n').next().unwrap_or_default();
    let before = PLACEHOLDER.replace_all(before, " ");
    let after = PLACEHOLDER.replace_all(after, " ");
    let skip = before.chars().count().saturating_sub(40);
    let near_before: String = before.chars().skip(skip).collect();
    let near_after: String = after.chars().take(20).collect();
    KEYWORD.is_match(&near_before) || KEYWORD.is_match(&near_after)
}
