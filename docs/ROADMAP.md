# Callus Roadmap

Every milestone is shippable, testable, and verifiable on its own.

---

## Milestone v0.1 — Local Core, 4 CLI Adapters & MCP

**Goal:** A working single Rust binary (`callus`) running locally on Windows, macOS, and Linux with SQLite (`FTS5` + `sqlite-vec` + graph), zero-GPU external `llama.cpp` support, client-side scrubbing, and all 4 CLI adapters.

### Deliverables
- [ ] Cargo workspace & crate structure (`callus-core`, `callus-store`, `callus-cli`)
- [ ] `KnowledgeStore` trait + **SQLite (WAL)** implementation (`FTS5` + `sqlite-vec` + `graph_nodes` / `graph_edges` + migrations)
- [ ] Client-side **privacy scrubber** (secrets, tokens, paths, usernames, IPs) + `callus scrub-test` + exhaustive unit tests
- [ ] External **`llama.cpp` HTTP embedding client** (`EMBEDDING_URL`, `EMBEDDING_MODEL`, `EMBEDDING_DIMS`, `EMBEDDING_API_KEY`) with strict timeout & graceful BM25+Graph fallback
- [ ] **Failure → Recovery pairing** engine + **closed-loop feedback** + hybrid ranking (`RRF(BM25, Vector) × Graph × Wilson × Decay`)
- [ ] Normalized **4-CLI hook adapters** (`claude`, `codex`, `antigravity`, `opencode`) + `callus init` (`--dry-run`, `--uninstall`)
- [ ] Official Rust MCP server via **`rmcp`** (`callus_search`, `callus_report`, `callus_feedback`, `callus_export_candidates`)
- [ ] `callus export` for token-budgeted `AGENTS.md` / `CLAUDE.md` / `GEMINI.md` diffs + deterministic `PreToolUse` guards
- [ ] GitHub Actions CI on **GitHub-hosted runners** (Linux, Windows, macOS: `cargo test`, `clippy`, `fmt`)

### Acceptance Criteria
- `callus hook` completes in **< 10 ms p95** locally (without blocking on remote embedder).
- Reproducing a Windows PowerShell heredoc failure followed by a working `python -c` command automatically pairs `ErrorPattern -[:RESOLVED_BY]-> Fix`, injects the fix on the next failure, and exports a clean `AGENTS.md` diff.
- Scrubber test suite proves zero secret/path leakage across realistic stderr fixtures.

---

## Milestone v0.2 — Central Docker Server (PostgreSQL + Sync)

**Goal:** Multi-user team backend deployed via Docker Compose on a LiteVM (`app` + tuned `pgvector/pgvector`), supporting 10+ simultaneous users with external `llama.cpp` embeddings.

### Deliverables
- [ ] **PostgreSQL** `KnowledgeStore` implementation (`sqlx` + `pgvector` + `tsvector`/`pg_trgm` + `graph_nodes`/`graph_edges` + migrations)
- [ ] `callus serve` (Axum REST API, Streamable HTTP MCP at `/mcp`, Bearer token auth, health/metrics endpoint)
- [ ] Bidirectional scoped sync (`callus sync`, `sync_outbox`, CRDT-style counter merges, `personal` / `repo` / `team` enforcement)
- [ ] Background consolidation worker (cluster dedup, pending embedding backfill, staleness/model-churn decay)
- [ ] Production **multi-stage Dockerfile** (minimal Linux image) + LiteVM-tuned **`docker-compose.yml`** (`app` + `postgres`, external `EMBEDDING_URL` env config)
- [ ] Concurrency & load benchmark (10+ simulated simultaneous users on LiteVM budget, verifying RAM ≤ 200 MB and p95 latency)

---

## Milestone v0.3 — Embedded Web UI (React 19 SPA)

**Goal:** Fast, high-craft Web UI embedded directly inside the `callus` binary (`rust-embed`) following the **information-in-1-second** principle.

### Deliverables
- [ ] React 19 + TypeScript + Vite + Tailwind CSS v4 + TanStack Query frontend (`web/`) embedded into Rust binary
- [ ] **Overview Dashboard:** Top recurring mistakes, retries saved, OS/shell/CLI/model breakdown, embedder & sync status
- [ ] **Patterns & Graph Explorer:** Instant hybrid search + interactive 1–2 hop graph view (`ErrorPattern → Context → Fixes`), Verify / Deprecate / Merge controls
- [ ] **Export Studio:** Live preview & copy/download for `AGENTS.md` / `CLAUDE.md` diffs (additions + stale prunes), `PreToolUse` guards, and rule packs
- [ ] **Privacy & Settings:** Live scrubber tester, scope overview, external `llama.cpp` status
- [ ] Visual screenshot review of all 4 screens (desktop + narrow viewport) before sign-off

---

## Milestone v0.4 — Rule Packs, Packaging & Public MIT Release

**Goal:** Frictionless public open-source launch.

### Deliverables
- [ ] Versioned, human-reviewable community rule packs (`windows-pwsh`, `macos-zsh`, `git-basics`, etc.) with `callus pack import/export`
- [ ] Cross-platform release binaries via GitHub Actions (Windows `.zip`, macOS `.tar.gz`, Linux musl/gnu `.tar.gz`) + GHCR Docker image (`ghcr.io/rennerdo30/callus`)
- [ ] Installers / package manager manifests (`cargo install`, Homebrew tap / Scoop manifest, Docker quickstart)
- [ ] Security audit of scrubber, auth, sync boundaries, and prompt-injection defenses on injected hook hints
