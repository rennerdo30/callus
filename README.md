# Callus

> Turn repeated AI coding-agent mistakes into automatic immunity.

**Callus** is a fast, local-first, self-learning mistake memory, guard, and rule compiler for AI coding CLIs. It captures tool failures and their recoveries across **Claude Code**, **OpenAI Codex CLI**, **Antigravity (`agy`)**, and **OpenCode**, builds a hybrid **vector + BM25 + knowledge graph** of what broke and what fixed it, injects live hints on repeat failures, and exports clean `PreToolUse` guards and token-budgeted `AGENTS.md` / `CLAUDE.md` / `GEMINI.md` improvements.

Run it **100% locally** with embedded SQLite and an embedded React 19 Web UI (`callus ui`), or connect your team to a **LiteVM Docker central server** backed by PostgreSQL (`pgvector`) and an external `llama.cpp` embedding endpoint.

---

## Quickstart

### 1. Build or Install

```bash
# Build the embedded React 19 Web UI + single Rust binary
(cd web && npm ci && npm run build)
cargo install --path crates/callus-cli --locked
```

### 2. Wire Up Your Coding CLIs

```bash
# Preview changes first (non-destructive, creates .bak backups)
callus init --dry-run

# Install hooks, plugins, and MCP config for Claude Code, Codex, Antigravity, and OpenCode
callus init

# Optionally import curated MIT community rule packs
callus pack import packs/windows-pwsh.json
callus pack import packs/git-basics.json
callus pack import packs/rust-cargo.json
```

### 3. Open the Embedded Local Web UI

```bash
callus ui --port 8080
# Open http://127.0.0.1:8080
```

Inspect top recurring hotspots, explore the interactive 1–2 hop knowledge graph, verify/deprecate fixes, preview `AGENTS.md` diffs, and test the privacy scrubber directly in your browser.

### 4. Export Clean Rules & Test Privacy Scrubbing

```bash
# Export repo-scoped or global rule diffs (zero AGENTS.md bloat)
callus export --format agents-md --scope all --budget-lines 40
callus export --format hooks

# Verify what the client-side scrubber redacts
echo "DB_PASS=supersecret at C:\Users\alice\app" | callus scrub-test
```

### 5. Self-Hosted Team Server (Docker Compose LiteVM, Zero GPU)

```bash
cp .env.example .env
# Edit .env with a strong POSTGRES_PASSWORD and CALLUS_API_TOKEN
# Optionally point EMBEDDING_URL at an external llama.cpp /v1/embeddings host
docker compose up -d

# Sync local non-personal learnings with the team server
callus sync --server http://your-server:8080 --token "$CALLUS_API_TOKEN"
```

---

## Why Callus?

1. **Passive capture across 4 CLIs:** Lightweight `< 5 ms` native hooks (`PreToolUse`, `PostToolUse`, `PostToolUseFailure`) + official Rust MCP server (`rmcp`, stdio and Streamable HTTP `/mcp`).
2. **Automatic failure → recovery learning:** Pairs a failed tool call with the succeeding recovery call in the same session, tagged by `(OS, shell, CLI, model, tool, repo)`.
3. **Repository-aware isolation & promotion:** New git-repo failures start strictly isolated as `repo`-scoped; generic patterns seen in **≥ 2 distinct repositories** automatically promote to `team`.
4. **Hybrid Vector + BM25 + Graph retrieval:** Fuses external `llama.cpp` cosine embeddings + FTS5/`tsvector` BM25 via RRF, expands 1–2 graph hops (`OCCURS_IN`, `RESOLVED_BY`, `SUPERSEDED_BY`, `OBSOLETE_IN`), and degrades seamlessly to BM25 + Graph offline.
5. **Mandatory client-side privacy scrubbing & prompt-injection defense:** Strips API keys, tokens, credentials, usernames, IPs, and absolute paths *before* local write or HTTP sync, and sanitizes every injected hook hint.
6. **Tiered outputs (zero `AGENTS.md` bloat):** Live failure hints (0 startup tokens), deterministic `PreToolUse` guards, and budgeted `AGENTS.md` / `CLAUDE.md` / `GEMINI.md` diffs with stale-rule prunes.

---

## Architecture at a Glance

| Mode | Runtime | Database | Embeddings | Target Footprint |
| :--- | :--- | :--- | :--- | :--- |
| **Local (Dev Machine)** | Single Rust binary (`callus`) | Embedded **SQLite (WAL)**: `FTS5` + `sqlite-vec` + graph tables | External `EMBEDDING_URL` (`llama.cpp`) or BM25+Graph offline fallback | < 30 MB RAM, < 5 ms hook latency |
| **Central Server (Team / LiteVM)** | Docker Compose (`app` + `postgres`) with embedded React 19 Web UI | **PostgreSQL** (`pgvector` + `tsvector`/`pg_trgm` + graph tables) | External `llama.cpp` server via `EMBEDDING_URL` (CPU or remote GPU) | ≤ 200 MB total RAM for 10+ concurrent users |

---

## Supported CLIs (Tier 1)

- **Claude Code** (`~/.claude/settings.json` hooks + MCP + `CLAUDE.md` / `AGENTS.md`)
- **OpenAI Codex CLI** (`~/.codex/config.toml` hooks + MCP + `AGENTS.md`)
- **Antigravity (`agy`)** (`hooks.json` + `mcp_config.json` + `AGENTS.md` / `GEMINI.md`)
- **OpenCode** (`opencode.json` MCP + thin TS plugin `tool.execute.before` / `tool.execute.after` + `AGENTS.md`)

---

## Documentation

- [Architecture Specification](docs/ARCHITECTURE.md) — Rust single binary, LiteVM performance budget, hybrid retrieval, scrubbing, tiered exports, and embedded Web UI
- [Data Model & Sync Protocol](docs/DATA_MODEL.md) — SQLite & PostgreSQL schemas, graph edges, hybrid scoring formula, and scoped sync
- [CLI Adapters](docs/CLI_ADAPTERS.md) — Exact hook, plugin, and MCP integration contracts for Claude Code, Codex, Antigravity, and OpenCode
- [Security Policy](SECURITY.md) — Threat model, privacy scrubbing guarantees, and prompt-injection defenses
- [Roadmap](docs/ROADMAP.md) — Milestones v0.1 through v0.4

---

## License

MIT — see [LICENSE](LICENSE).
