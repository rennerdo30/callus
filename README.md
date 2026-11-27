# Callus

> Turn repeated AI coding-agent mistakes into automatic immunity.

**Callus** is a fast, local-first, self-learning mistake memory, guard, and rule compiler for AI coding CLIs. It captures tool failures and their recoveries across **Claude Code**, **OpenAI Codex CLI**, **Antigravity (`agy`)**, and **OpenCode**, builds a hybrid **vector + BM25 + knowledge graph** of what broke and what fixed it, injects live hints on repeat failures, and exports clean `PreToolUse` guards and token-budgeted `AGENTS.md` / `CLAUDE.md` improvements.

Run it **100% locally** with embedded SQLite, or connect your team to a **lightweight Docker central server** backed by PostgreSQL (`pgvector`) and an external `llama.cpp` embedding endpoint.

---

## Why Callus?

Coding agents repeatedly fall into the same environment and workflow traps—Python heredocs on Windows PowerShell, `zsh` quoting and globbing, repo-specific CLI flags, build-tool quirks—burning tokens and time every session.

Callus fixes that without bloating your startup context:

1. **Passive capture across 4 CLIs:** Lightweight `<5 ms` native hooks (`PreToolUse`, `PostToolUse`, `PostToolUseFailure`) + MCP server (`rmcp`).
2. **Automatic failure → recovery learning:** Pairs a failed tool call with the succeeding recovery call in the same session, tagged by `(OS, shell, CLI, model, tool, repo)`.
3. **Hybrid Vector + BM25 + Graph retrieval:** Matches fuzzy errors via embeddings + full-text search, then traverses the context graph (`OCCURS_IN`, `RESOLVED_BY`, `SUPERSEDED_BY`, `OBSOLETE_IN`) to return the highest-confidence fix for your exact environment.
4. **Closed-loop feedback & staleness decay:** Automatically boosts fixes that resolve retries, demotes fixes that fail, and flags obsolete rules when newer models stop hitting them.
5. **Client-side privacy scrubbing:** Strips API keys, tokens, usernames, IPs, and absolute paths *before* anything is persisted or synced.
6. **Tiered exports (zero `AGENTS.md` bloat):**
   - **Layer 1:** Instant `PostToolUseFailure` hint injection (0 startup tokens).
   - **Layer 2:** Deterministic `PreToolUse` guard rules for mechanical shell/OS pitfalls.
   - **Layer 3:** Token-budgeted `AGENTS.md` / `CLAUDE.md` / `GEMINI.md` diffs with frequency counts and prune recommendations.

---

## Architecture at a Glance

| Mode | Runtime | Database | Embeddings | Target Footprint |
| :--- | :--- | :--- | :--- | :--- |
| **Local (Dev Machine)** | Single Rust binary (`callus`) | Embedded **SQLite (WAL)**: `FTS5` + `sqlite-vec` + graph tables | External `EMBEDDING_URL` (`llama.cpp`) or BM25+Graph offline fallback | < 30 MB RAM, < 5 ms hook latency |
| **Central Server (Team / LiteVM)** | Docker Compose (`app` + `postgres`) with embedded React Web UI | **PostgreSQL** (`pgvector` + `tsvector`/`pg_trgm` + graph tables) | External `llama.cpp` server via `EMBEDDING_URL` (CPU or remote GPU) | ~125 MB total RAM for 10+ concurrent users |

---

## Supported CLIs (Tier 1)

- **Claude Code** (`~/.claude/settings.json` hooks + MCP + `CLAUDE.md` / `AGENTS.md`)
- **OpenAI Codex CLI** (`~/.codex/config.toml` hooks + MCP + `AGENTS.md`)
- **Antigravity (`agy`)** (`hooks.json` + `mcp_config.json` / native plugin bundle + `AGENTS.md` / `GEMINI.md`)
- **OpenCode** (`opencode.json` MCP + thin TS plugin `tool.execute.before` / `tool.execute.after` + `AGENTS.md`)

---

## Documentation

- [Architecture Specification](docs/ARCHITECTURE.md) — Rust single binary, LiteVM performance budget, hybrid retrieval, scrubbing, tiered exports, and embedded Web UI
- [Data Model & Sync Protocol](docs/DATA_MODEL.md) — SQLite & PostgreSQL schemas, graph edges, hybrid scoring formula, and scoped sync
- [CLI Adapters](docs/CLI_ADAPTERS.md) — Exact hook, plugin, and MCP integration contracts for Claude Code, Codex, Antigravity, and OpenCode
- [Roadmap](docs/ROADMAP.md) — Phased implementation plan from v0.1 Local MVP to v0.4 Public Release

---

## License

MIT — see [LICENSE](LICENSE).
