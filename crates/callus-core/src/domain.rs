//! Normalized domain model. See `docs/DATA_MODEL.md`.
//!
//! Timestamps are ISO-8601 UTC strings; ids are ULID/UUID strings.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::CallusError;

/// Declares an enum whose serde, `Display` and `FromStr` forms are the spec strings.
macro_rules! string_enum {
    ($(#[$doc:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant,)+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = CallusError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($text => Ok(Self::$variant),)+
                    _ => Err(CallusError::UnknownVariant {
                        kind: stringify!($name),
                        value: value.to_owned(),
                    }),
                }
            }
        }
    };
}

string_enum!(
    /// Source coding CLI.
    CliKind { ClaudeCode = "claude-code", Codex = "codex", Antigravity = "antigravity", OpenCode = "opencode" }
);
string_enum!(OsKind { Windows = "windows", MacOs = "macos", Linux = "linux" });
string_enum!(ShellKind {
    Pwsh = "pwsh", Cmd = "cmd", Bash = "bash", Zsh = "zsh", Fish = "fish", Unknown = "unknown"
});
string_enum!(ToolPhase { Pre = "pre", PostOk = "post_ok", PostFailure = "post_failure" });
string_enum!(
    /// Visibility scope of a pattern or fix.
    Scope { Personal = "personal", Repo = "repo", Team = "team", PublicPack = "public-pack" }
);
string_enum!(FixType { Hint = "hint", PreToolGuard = "pre_tool_guard", AgentsMdRule = "agents_md_rule" });
string_enum!(FixStatus {
    Candidate = "candidate", Verified = "verified", Deprecated = "deprecated", Obsolete = "obsolete"
});
string_enum!(FixSource {
    AutoRecovery = "auto_recovery", AgentReport = "agent_report", Human = "human", Pack = "pack"
});
string_enum!(EmbeddingStatus { Ready = "ready", Pending = "pending", Skipped = "skipped" });
string_enum!(NodeKind {
    ErrorPattern = "error_pattern", Fix = "fix", Environment = "environment", Cli = "cli",
    Model = "model", Tool = "tool", Repo = "repo", Rule = "rule"
});
string_enum!(EdgeType {
    OccursIn = "OCCURS_IN", ResolvedBy = "RESOLVED_BY", SupersededBy = "SUPERSEDED_BY",
    Causes = "CAUSES", ConflictsWith = "CONFLICTS_WITH", ObsoleteIn = "OBSOLETE_IN",
    CompiledTo = "COMPILED_TO"
});

/// One hook invocation, normalized across CLIs. Scrub before storing or syncing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolEvent {
    pub event_id: String,
    pub timestamp: String,
    pub cli: CliKind,
    pub cli_version: Option<String>,
    pub model: Option<String>,
    /// Hashed by the adapter.
    pub session_id: String,
    pub os: OsKind,
    pub shell: ShellKind,
    pub repo_hash: Option<String>,
    pub tool_name: String,
    pub phase: ToolPhase,
    pub command_or_input: Option<String>,
    pub exit_code: Option<i32>,
    pub error_text: Option<String>,
    pub duration_ms: Option<u64>,
}

/// Canonical failure cluster (`error_patterns`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorPattern {
    pub id: String,
    pub signature_hash: String,
    pub title: String,
    pub error_template: String,
    pub tool_name: String,
    pub scope: Scope,
    pub repo_hash: Option<String>,
    pub occurrence_count: u64,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub embedding_status: EmbeddingStatus,
    pub updated_at: String,
}

/// Candidate or verified resolution (`fixes`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fix {
    pub id: String,
    pub summary: String,
    pub fix_type: FixType,
    pub pattern_before: Option<String>,
    pub pattern_after: Option<String>,
    pub guard_regex: Option<String>,
    pub status: FixStatus,
    pub scope: Scope,
    pub source: FixSource,
    pub created_at: String,
    pub updated_at: String,
}

/// Typed knowledge-graph node (`graph_nodes`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    pub props_json: serde_json::Value,
}

/// Typed edge (`graph_edges`), keyed by `(src_id, edge_type, dst_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub src_id: String,
    pub edge_type: EdgeType,
    pub dst_id: String,
    pub worked_count: u64,
    pub failed_count: u64,
    pub hit_count: u64,
    pub last_seen_at: String,
}

/// Short-retention local session record (`episodes`). Never synced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub id: String,
    pub session_id: String,
    pub cli: CliKind,
    pub model: Option<String>,
    pub os: OsKind,
    pub shell: ShellKind,
    pub tool_name: String,
    pub phase: ToolPhase,
    pub pattern_id: Option<String>,
    pub injected_fix_id: Option<String>,
    pub command_scrubbed: Option<String>,
    pub created_at: String,
}

/// Caller environment used for context boosts during ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextInfo {
    pub os: OsKind,
    pub shell: ShellKind,
    pub cli: CliKind,
    pub model: Option<String>,
    pub repo_hash: Option<String>,
}

/// Lookup for fixes matching an error, command or task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub tool_name: Option<String>,
    pub context: Option<ContextInfo>,
    pub limit: usize,
}

/// A fix with its ranking result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedFix {
    pub fix: Fix,
    pub pattern_id: String,
    /// Final score after context, confidence and recency weighting.
    pub score: f64,
    /// Wilson score lower bound.
    pub confidence: f64,
    pub worked_count: u64,
    pub failed_count: u64,
}
