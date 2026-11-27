# Callus Architecture Specification

**Status:** Design Baseline (v0.1)  
**License:** MIT  
**Target Environment:** Local developer machines (Windows / macOS / Linux) + LiteVM Docker team server (10+ simultaneous users, zero GPU required)

---

## 1. Problem Statement & Core Insight

AI coding agents repeatedly hit the same environment, shell, and workflow mistakes across sessions and machines—for example:
- Python heredocs failing under Windows PowerShell
- `zsh` globbing and quoting errors
- Platform-specific CLI flags, path separators, or build-tool quirks
- Repository-specific pitfalls that another developer's agent already solved yesterday

Most existing agent-memory tools either:
1. Depend on manual user corrections ("no, don't do that") instead of passive failure telemetry,
2. Dump everything into `CLAUDE.md` / `AGENTS.md` (causing startup token bloat and rule drift), or
3. Require heavy multi-database / GPU infrastructure that is impractical to self-host on a small VM or adopt at work.

**Callus** treats recurring coding-agent mistakes as **structured failure-and-recovery telemetry** backed by a lightweight **hybrid vector + BM25 + knowledge graph** engine.

---

## 2. High-Level Architecture

A **single Rust binary** (`callus`) powers every component:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        Developer Machine (Local)                        │
│                                                                         │
│  Claude Code ──┐                                                        │
│  Codex CLI  ───┼─► `callus hook` (<5 ms) ──► Client-Side Scrubber       │
│  Antigravity ──┤                                    │                   │
│  OpenCode   ───┘   `callus mcp` (stdio) ────────────┤                   │
│                                                     ▼                   │
│                                          Local SQLite (WAL)             │
│                                     FTS5 + sqlite-vec + Graph           │
│                                                     │                   │
└─────────────────────────────────────────────────────┼───────────────────┘
                                                      │ Optional Sync
                                                      │ (HTTPS, scoped)
┌─────────────────────────────────────────────────────▼───────────────────┐
│                  Central Server (LiteVM Docker Compose)                 │
│                                                                         │
│  ┌───────────────────────────────────────┐    ┌──────────────────────┐  │
│  │  `callus serve` (Rust / Axum / rmcp)  │◄──►│ PostgreSQL (pgvector │  │
│  │  • REST API + Sync API                │    │ + tsvector + Graph)  │  │
│  │  • Streamable HTTP MCP (`/mcp`)       │    └──────────────────────┘  │
│  │  • Embedded React 19 Web UI (`/`)     │                              │
│  └──────────────────┬────────────────────┘                              │
│                     │ Optional HTTP `/v1/embeddings`                    │
└─────────────────────┼───────────────────────────────────────────────────┘
                      ▼
       External `llama.cpp` Server (CPU or Remote GPU, env-configured)
```

### Subcommands of `callus`

| Command | Purpose |
| :--- | :--- |
| `callus init [--claude] [--codex] [--antigravity] [--opencode]` | Auto-detects installed CLIs and installs hooks/plugins + MCP config cleanly (idempotent, `--dry-run`, `--uninstall`). |
| `callus hook --cli <cli> --event <pre\|post\|failure\|stop>` | Ultra-fast (`<5 ms`) stdin/stdout hook runner: scrubs event, checks `PreToolUse` guards, records failures/recoveries, injects proven fixes on failure. |
| `callus mcp` | Runs the official Rust MCP server (`rmcp`) over stdio (or connects to remote server). |
| `callus serve` | Runs the HTTP API, Streamable HTTP MCP (`/mcp`), team sync endpoints, background consolidator, and embedded React SPA. |
| `callus sync` | Bidirectional scoped sync with a configured central server (queues offline writes safely). |
| `callus export [--format agents-md\|claude-md\|hooks\|pack]` | Generates token-budgeted rule diffs, deterministic guard hooks, and anonymized rule packs. |
| `callus scrub-test` | Lets users test redaction rules on sample text/commands before enabling sync. |

---

## 3. Tech Stack & Performance Budget

### Stack Choices (Nov 2026)

- **Core Runtime:** Rust (`tokio`, `axum`, `serde`, `clap`)
- **MCP Protocol:** Official Rust SDK **`rmcp`** (stdio for local CLI + Streamable HTTP for central server)
- **Local Storage:** **SQLite (WAL)** via `rusqlite` (bundled) + `FTS5` + `sqlite-vec` + relational graph tables (`nodes`, `edges`)
- **Central Server Storage:** **PostgreSQL** (`pgvector/pgvector:pg17`) via `sqlx` + `pgvector` + `tsvector` + `pg_trgm` + relational graph tables
- **Embeddings:** External OpenAI-compatible HTTP client targeting **`llama.cpp` (`llama-server --embedding`)** via environment variables—zero GPU or C++ ML runtime inside the Callus binary/container
- **Frontend:** **React 19 + TypeScript + Vite + Tailwind CSS v4 + TanStack Query**, compiled and embedded directly into the Rust binary via `rust-embed` (zero Nginx container)

### LiteVM Resource Budget (10+ Simultaneous Users)

| Component | Target Idle RAM | Target Active RAM (10 users) | Latency Budget |
| :--- | :--- | :--- | :--- |
| `callus hook` (local CLI) | — | < 15 MB transient | **< 5 ms** (p95 < 10 ms) |
| `callus serve` (Docker `app`) | ~20–30 MB | ~40–60 MB | **< 15 ms** p95 query (excluding external embedder RTT) |
| `postgres` (tuned LiteVM) | ~60–80 MB | ~100–120 MB | `shared_buffers=128MB`, `work_mem=4MB`, `max_connections=32` |
| **Total Docker Stack** | **~85–110 MB** | **~140–180 MB** | Fits comfortably on a **1 vCPU / 512 MB–1 GB LiteVM** |

---

## 4. External `llama.cpp` Embedding Integration

Callus never embeds a heavy local GPU stack. Embeddings are configured purely via environment variables:

```env
# Required for vector search; omit or set EMBEDDING_PROVIDER=none for pure BM25 + Graph mode
EMBEDDING_PROVIDER=openai-compatible   # "openai-compatible" | "none"
EMBEDDING_URL=http://llama-host:8081/v1/embeddings
EMBEDDING_MODEL=bge-small-en-v1.5      # Sent in JSON payload (or ignored by single-model llama-server)
EMBEDDING_DIMS=384                     # Must match vector column dimension (e.g. 384, 768, 1024)
EMBEDDING_API_KEY=                     # Optional Bearer token
EMBEDDING_TIMEOUT_MS=1500              # Strict timeout so hooks/queries never stall
```

**Graceful degradation rules:**
1. If `EMBEDDING_URL` is unset or unreachable, Callus **never fails a hook or search**—it falls back seamlessly to `BM25/FTS + normalized error-signature hash + knowledge graph`.
2. Unembedded patterns are marked `embedding_status = 'pending'` and backfilled asynchronously by the background worker once `llama.cpp` is reachable.
3. Dimension mismatches are caught at startup with a clear actionable error (never silent corruption).

---

## 5. Self-Learning Engine

### 5.1 Automatic Failure → Recovery Pairing
Agents rarely remember to self-report mistakes via an MCP tool. Callus captures the real ground-truth signal automatically:
1. Tool call `T1` fails (non-zero exit code or error payload) in session `S`.
2. Callus records an episode in `raw_events` (scrubbed) and creates/matches an `ErrorPattern` node.
3. Within the next `N` turns (default: `3`) or `T` seconds (default: `180s`) in session `S`, if a tool call `T2` with overlapping intent (same tool, target file, command stem, or working directory) **succeeds**:
   - Callus extracts the **delta** (`failed_cmd → working_cmd` or `failed_pattern → recovery_pattern`).
   - It creates or reinforces `(ErrorPattern) -[:RESOLVED_BY]-> (Fix)` with provenance tied to `(OS, shell, CLI, model, repo)`.

### 5.2 Closed-Loop Feedback on Injected Hints
1. When `PostToolUseFailure` fires and Callus injects a candidate `Fix` into `additionalContext`, Callus records `injected_fix_id` for that session.
2. If the very next tool call succeeds → increment `worked_count` on the `RESOLVED_BY` edge.
3. If the next tool call fails with the same error signature → increment `failed_count` and avoid re-injecting the same hint in a loop.
4. Confidence is computed via **Wilson score lower bound** so 1 lucky hit never outranks a battle-tested fix with 25/26 successes.

### 5.3 Hybrid Retrieval (Vector + BM25 → Graph Expansion)
1. **Candidate generation:** Run `BM25` (`FTS5` / `tsvector`) + `Vector` (`sqlite-vec` / `pgvector`) and fuse with **Reciprocal Rank Fusion (RRF)**, plus exact normalized error-signature match boost.
2. **Graph expansion & filtering:** For top candidate `ErrorPattern` nodes, traverse 1–2 hops across `OCCURS_IN`, `RESOLVED_BY`, `SUPERSEDED_BY`, and `OBSOLETE_IN`.
3. **Context boost:** Boost fixes proven in the exact `(os, shell, cli, model, repo)` context; penalize cross-OS mismatches (e.g., do not suggest a `zsh` fix on `pwsh`).
4. **Model-churn decay:** Patterns that stop occurring on current models/CLIs decay over time and are surfaced as **prune candidates** for `AGENTS.md` / `CLAUDE.md`.

### 5.4 Background Consolidation
Runs asynchronously (never on the hook hot path):
- Deduplicates near-identical `ErrorPattern` clusters
- Backfills pending embeddings from external `llama.cpp`
- Flags contradictory fixes (`CONFLICTS_WITH`)
- Marks stale rules (`OBSOLETE_IN`)
- Optionally calls a remote LLM API (or lets the user's own agent pull raw clusters via MCP `export_candidates`) to draft clean prose rules and guard scripts

---

## 6. Privacy, Scrubbing & Scoped Sync

### Mandatory Client-Side Scrubber
Runs **locally before SQLite insert and before any HTTP sync**:
- **Secrets:** AWS keys, GitHub/GitLab PATs, OpenAI/Anthropic/Google API keys, Bearer/Basic auth headers, JWTs, private key blocks, `.env` assignments (`KEY=value`), high-entropy strings near secret keywords
- **Paths:** Repo root → `<REPO>`, home dir → `<HOME>`, Windows/Unix user paths (`C:\Users\<name>` / `/home/<name>` / `/Users/<name>`) → `<HOME>`
- **Network/Identity:** Internal IPs, email addresses, machine hostnames, custom user-defined regex deny-list

### Visibility Scopes
Every pattern/fix carries an explicit scope:
- `personal` — stays strictly in local SQLite, never synced
- `repo` — syncs only to the team server tagged for that repository hash
- `team` — syncs across the organization's self-hosted server (default for generic OS/shell/toolchain lessons)
- `public-pack` — explicit opt-in export format for curated, human-reviewed community packs (e.g., `windows-pwsh.json`, `macos-zsh.json`)

---

## 7. Tiered Outputs (Zero `AGENTS.md` Bloat)

Instead of dumping every pitfall into startup markdown files:
1. **Layer 1 — Live Failure Hints (`PostToolUseFailure`):** Injected *only* when a known error occurs. **0 startup tokens.**
2. **Layer 2 — Deterministic `PreToolUse` Guards:** Mechanical, high-confidence shell/OS rules (e.g., PowerShell heredoc warning/block, bad `zsh` unquoted globbing, repo-specific forbidden flags) enforced before execution.
3. **Layer 3 — Budgeted `AGENTS.md` / `CLAUDE.md` / `GEMINI.md` Diffs:** Ranked by `frequency × impact × confidence`, capped to a configurable line/token budget, including **stale-rule removal suggestions**.

---

## 8. Web UI (Embedded React SPA)

Served directly from `callus serve` at `/` (and optionally `callus ui` locally). Built on the **information-in-1-second** principle—4 clean views, no empty cards or decorative fluff:

1. **Overview (Health & Hotspots):** Top recurring failures this week, retries prevented, breakdown by OS / shell / CLI / model, embedder & sync status badge.
2. **Patterns & Graph Explorer:** Instant hybrid search across error clusters + interactive 1–2 hop neighborhood graph (`ErrorPattern → Contexts → Fixes → Rules`), one-click Verify / Deprecate / Promote actions.
3. **Export Studio:** Live preview & copy/download for `AGENTS.md` / `CLAUDE.md` diffs (additions + prunes), `PreToolUse` guard configs, and anonymized rule packs.
4. **Settings, Scrubber & Sync:** Live scrubber playground (paste a messy stderr to verify redaction), scope controls, connected clients/CLIs, external `llama.cpp` health check.
