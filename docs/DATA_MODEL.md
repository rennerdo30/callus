# Callus Data Model & Sync Protocol

Callus implements a single Rust `KnowledgeStore` trait backed by:
- **SQLite (WAL)** for local developer installations (`FTS5` + `sqlite-vec` + graph tables)
- **PostgreSQL** for the central Docker server (`tsvector`/`pg_trgm` + `pgvector` + graph tables)

Both engines share the same logical schema and deterministic scoring semantics.

---

## 1. Normalized Tool Event (Input Contract)

Every CLI adapter (`claude`, `codex`, `antigravity`, `opencode`) normalizes hook stdin payloads into this internal event before scrubbing:

```json
{
  "event_id": "01J...",
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
}
```

> **Note:** `command_or_input`, `error_text`, and any path fields are passed through the **client-side scrubber** before reaching storage or sync.

---

## 2. Core Logical Schema (SQLite & PostgreSQL)

### 2.1 `error_patterns` (Canonical Failure Clusters)

| Column | Type | Notes |
| :--- | :--- | :--- |
| `id` | `TEXT` (ULID/UUID) PK | Stable global ID for sync |
| `signature_hash` | `TEXT` UNIQUE | SHA-256 of normalized error template (paths/numbers/UUIDs stripped) |
| `title` | `TEXT` | Short 1-line summary (e.g., `PowerShell rejects bash << heredoc operator`) |
| `error_template` | `TEXT` | Scrubbed canonical error message |
| `tool_name` | `TEXT` | Normalized tool (`shell`, `edit`, `read`, `mcp:<name>`, etc.) |
| `scope` | `TEXT` | `personal` \| `repo` \| `team` \| `public-pack` |
| `repo_hash` | `TEXT` NULL | Present when `scope = 'repo'` |
| `occurrence_count` | `INTEGER` | Total observed hits |
| `first_seen_at` | `TIMESTAMPTZ` / `TEXT` | ISO-8601 UTC |
| `last_seen_at` | `TIMESTAMPTZ` / `TEXT` | ISO-8601 UTC (used for recency decay) |
| `embedding_status` | `TEXT` | `ready` \| `pending` \| `skipped` |
| `updated_at` | `TIMESTAMPTZ` / `TEXT` | Monotonic sync watermark |

**Search indexes:**
- **SQLite:** `error_patterns_fts` (`FTS5` over `title`, `error_template`, `tool_name`) + `error_patterns_vec` (`sqlite-vec` `vec0` table with configured `EMBEDDING_DIMS`)
- **PostgreSQL:** `search_tsv` (`GENERATED ALWAYS AS (to_tsvector('english', ...)) STORED`, GIN index) + `pg_trgm` GIN index + `embedding vector(DIMS)` (HNSW cosine index)

---

### 2.2 `fixes` (Candidate & Verified Resolutions)

| Column | Type | Notes |
| :--- | :--- | :--- |
| `id` | `TEXT` PK | Stable global ID |
| `summary` | `TEXT` | Concise actionable fix shown to the agent (≤ 240 chars) |
| `fix_type` | `TEXT` | `hint` \| `pre_tool_guard` \| `agents_md_rule` |
| `pattern_before` | `TEXT` NULL | Scrubbed failing command/pattern snippet |
| `pattern_after` | `TEXT` NULL | Scrubbed working command/pattern snippet |
| `guard_regex` | `TEXT` NULL | Optional deterministic matcher for `PreToolUse` guards |
| `status` | `TEXT` | `candidate` \| `verified` \| `deprecated` \| `obsolete` |
| `scope` | `TEXT` | `personal` \| `repo` \| `team` \| `public-pack` |
| `source` | `TEXT` | `auto_recovery` \| `agent_report` \| `human` \| `pack` |
| `created_at` | `TIMESTAMPTZ` / `TEXT` | ISO-8601 UTC |
| `updated_at` | `TIMESTAMPTZ` / `TEXT` | ISO-8601 UTC |

---

### 2.3 Knowledge Graph Tables (`graph_nodes` & `graph_edges`)

Instead of maintaining a separate Neo4j instance, Callus stores typed nodes and edges directly in SQLite / PostgreSQL and executes 1–3 hop traversals via indexed joins and recursive CTEs.

#### `graph_nodes`
| Column | Type | Notes |
| :--- | :--- | :--- |
| `id` | `TEXT` PK | e.g., `env:windows:pwsh`, `cli:claude-code`, `model:gemini-4-neon`, `err:<id>`, `fix:<id>`, `repo:<hash>` |
| `kind` | `TEXT` | `error_pattern` \| `fix` \| `environment` \| `cli` \| `model` \| `tool` \| `repo` \| `rule` |
| `label` | `TEXT` | Human-readable display label |
| `props_json` | `TEXT` / `JSONB` | Lightweight typed attributes |

#### `graph_edges`
| Column | Type | Notes |
| :--- | :--- | :--- |
| `src_id` | `TEXT` | FK → `graph_nodes.id` |
| `edge_type` | `TEXT` | `OCCURS_IN` \| `RESOLVED_BY` \| `SUPERSEDED_BY` \| `CAUSES` \| `CONFLICTS_WITH` \| `OBSOLETE_IN` \| `COMPILED_TO` |
| `dst_id` | `TEXT` | FK → `graph_nodes.id` |
| `worked_count` | `INTEGER` | Closed-loop successes (for `RESOLVED_BY`) |
| `failed_count` | `INTEGER` | Closed-loop failures (for `RESOLVED_BY`) |
| `hit_count` | `INTEGER` | Co-occurrence count (for `OCCURS_IN`) |
| `last_seen_at` | `TIMESTAMPTZ` / `TEXT` | Recency tracking |
| `PRIMARY KEY` | `(src_id, edge_type, dst_id)` | Idempotent upserts & clean merge during sync |

Indexes on `(src_id, edge_type)` and `(dst_id, edge_type)` keep 1–2 hop lookups sub-millisecond even at 100k+ edges.

---

### 2.4 `episodes` (Short-Retention Local Session Buffer)

Used strictly for **Failure → Recovery pairing** and **hint feedback tracking** (auto-pruned after e.g. 7 days locally, never synced raw):

| Column | Type | Notes |
| :--- | :--- | :--- |
| `id` | `TEXT` PK | Event ID |
| `session_id` | `TEXT` | Hashed session ID |
| `cli` | `TEXT` | Source CLI |
| `model` | `TEXT` | Model name |
| `os` | `TEXT` | `windows` \| `macos` \| `linux` |
| `shell` | `TEXT` | `pwsh` \| `cmd` \| `bash` \| `zsh` \| `fish` \| `unknown` |
| `tool_name` | `TEXT` | Normalized tool name |
| `phase` | `TEXT` | `pre` \| `post_ok` \| `post_failure` |
| `pattern_id` | `TEXT` NULL | Matched `error_patterns.id` |
| `injected_fix_id` | `TEXT` NULL | Fix injected into context on failure |
| `command_scrubbed` | `TEXT` NULL | Scrubbed input snippet |
| `created_at` | `TIMESTAMPTZ` / `TEXT` | Timestamp |

---

## 3. Hybrid Ranking Formula

When looking up fixes for a live failure or query:

1. **Candidate Recall:**
   - Exact `signature_hash` match (`score_exact = 1.0` if hit)
   - `BM25` top-20 (`rank_bm25`)
   - `Vector` cosine top-20 (`rank_vec`, skipped cleanly if embedder disabled/offline)
   - Fused relevance via Reciprocal Rank Fusion:
     $$\text{RRF}(p) = \frac{w_{\text{bm25}}}{k + \text{rank}_{\text{bm25}}} + \frac{w_{\text{vec}}}{k + \text{rank}_{\text{vec}}} + w_{\text{exact}} \cdot \mathbb{I}[\text{exact}]$$
     *(default $k = 60$)*

2. **Graph Context Multiplier (per candidate fix $f$ linked via `RESOLVED_BY`):**
   - Exclude fixes with outgoing `SUPERSEDED_BY` or `status IN ('deprecated', 'obsolete')`.
   - Exclude fixes marked `OBSOLETE_IN` for the caller's `(cli, model)`.
   - Multiply by context match weights:
     - Same `os` + `shell`: `× 1.4` (different OS for a shell error: `× 0.3`)
     - Same `repo_hash`: `× 1.3`
     - Same `cli` / `model`: `× 1.15`

3. **Confidence & Recency:**
   - **Wilson score lower bound** ($z = 1.96$) over `(worked_count, worked_count + failed_count)` (with a small prior for human-verified fixes).
   - **Exponential recency decay** (half-life default: 45 days) so stale workarounds naturally yield to fresh ones.

4. **Injection Gate:**
   - `PostToolUseFailure` only injects a hint when final score exceeds `min_inject_confidence` (default `0.45`)—preventing noisy or irrelevant hints from distracting the agent.

---

## 4. Local ↔ Central Server Sync Protocol

- **Transport:** HTTPS JSON (`/api/v1/sync/push` and `/api/v1/sync/pull?since=<cursor>`) with Bearer API token auth.
- **What syncs:** Only scrubbed `error_patterns`, `fixes`, and `graph_nodes`/`graph_edges` with `scope IN ('repo', 'team')`. Raw `episodes` and `scope = 'personal'` records **never leave the laptop**.
- **Conflict resolution (CRDT-style):**
  - Counter fields (`occurrence_count`, `worked_count`, `failed_count`, `hit_count`) merge by **delta accumulation** per client cursor (never overwritten).
  - Human status transitions (`verified` / `deprecated`) use last-write-wins with `updated_at` + audit trail.
- **Offline safety:** Local SQLite queues outbound deltas in `sync_outbox` and flushes opportunistically when the server is reachable.
