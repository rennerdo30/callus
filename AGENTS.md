# Callus Agent Rules

Read `E:/Development/AGENTS.md` and `E:/Development/general-agent-rules.md` first. Every rule there applies here alongside the repository invariants below.

## Project Identity & Scope

- **Project:** Callus (`github.com/rennerdo30/callus`), MIT-licensed public open-source product, also designed for self-hosted team use at work.
- **Pair Desk:** slug `callus`, prefix `CL`. Keep handoff, issues, plans, and progress on Pair Desk.
- **Public repository CI:** Always use **GitHub-hosted runners** for CI, docs, and release builds (never self-hosted runners).
- **Paths & branches:** Write `E:` paths everywhere (never new `D:\` paths). Keep active unmerged branches ≤ 10 (hard max 20); keep primary checkout (`E:/Development/callus`) on `main`.

## Non-Negotiable Architecture Invariants

1. **Single Rust binary** (`callus`) for CLI, hooks, stdio/HTTP MCP (`rmcp`), central HTTP server (`axum`), and embedded React Web UI (`rust-embed`).
2. **Strict DB split behind one `KnowledgeStore` trait:**
   - **Local mode:** Embedded **SQLite (WAL)** (`FTS5` + `sqlite-vec` + `nodes`/`edges` graph tables).
   - **Central Docker server:** Always **PostgreSQL** (`pgvector` + `tsvector`/`pg_trgm` + `nodes`/`edges` graph tables).
3. **Zero local GPU requirement; external `llama.cpp` embeddings:**
   - Never bundle PyTorch, CUDA, or heavyweight local GPU runtimes into the Docker image.
   - Embeddings talk to an external OpenAI-compatible `/v1/embeddings` endpoint (`llama.cpp` server) configured via env vars (`EMBEDDING_URL`, `EMBEDDING_MODEL`, `EMBEDDING_API_KEY`, `EMBEDDING_DIMS`).
   - System MUST degrade gracefully to BM25 + normalized signature + graph retrieval when `EMBEDDING_URL` is unset or unreachable.
4. **LiteVM performance budget (10+ concurrent users):**
   - Hook invocation latency (`callus hook ...`): **< 10 ms p95** (target < 5 ms).
   - Central server stack (`app` + tuned `postgres` in Docker Compose): target **≤ 200 MB total RAM** idle/nominal on a 1 vCPU LiteVM.
5. **Four Tier-1 CLIs from day one:** Claude Code, OpenAI Codex CLI, Antigravity (`agy`), and OpenCode. All normalize into one internal `ToolEvent` schema.
6. **Mandatory client-side privacy scrubbing:** Secrets, tokens, credentials, usernames, IPs, and absolute paths are scrubbed *before* local persistence or network sync. Never ship raw unredacted work/internal telemetry.
7. **Information in 1 second (Web UI):** Every Web UI view shows key state first, visible hierarchy, zero clutter or empty filler, and is visually verified by screenshot before owner review.
8. **No slop, no public leaks:** Never commit internal work hostnames, private repo names, credentials, or `.pair-desk.json` into this public repository.

## Documentation Map

- `docs/ARCHITECTURE.md` — full system design, LiteVM budgets, hybrid search, self-learning loop, exports, and Web UI
- `docs/DATA_MODEL.md` — SQLite & PostgreSQL schemas, graph model, scoring math, and sync protocol
- `docs/CLI_ADAPTERS.md` — hook/plugin/MCP contracts for Claude Code, Codex, Antigravity, and OpenCode
- `docs/ROADMAP.md` — milestone breakdown (v0.1 through v0.4)
