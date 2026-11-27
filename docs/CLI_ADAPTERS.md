# Callus CLI Adapters (Tier-1 Support)

Callus supports **four Tier-1 coding CLIs** out of the box. Every adapter translates CLI-specific hook/plugin payloads into Callus's unified `ToolEvent` format and installs cleanly via `callus init`.

---

## 1. Summary Matrix

| CLI | Hook / Plugin Mechanism | Hook Config File | MCP Config File | Rule Export Target |
| :--- | :--- | :--- | :--- | :--- |
| **Claude Code** | Native `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `Stop` | `~/.claude/settings.json` (or `.claude/settings.json`) | `~/.claude.json` / `.mcp.json` | `CLAUDE.md` / `AGENTS.md` |
| **OpenAI Codex CLI** | Native `PreToolUse`, `PostToolUse` | `~/.codex/config.toml` (or `.codex/config.toml`) | `[mcp_servers.callus]` in `config.toml` | `AGENTS.md` |
| **Antigravity (`agy`)** | Native `PreToolUse`, `PostToolUse`, `Stop` (camelCase JSON; shippable as plugin bundle) | `~/.gemini/config/hooks.json` (or `.agents/hooks.json`) | `~/.gemini/config/mcp_config.json` | `AGENTS.md` / `GEMINI.md` |
| **OpenCode** | Thin TS plugin (`tool.execute.before`, `tool.execute.after`) | `~/.config/opencode/plugins/callus.ts` + `opencode.json` | `"mcp.callus"` in `opencode.json` | `AGENTS.md` |

---

## 2. Per-CLI Contract Details

### 2.1 Claude Code
- **Events wired:**
  - `PreToolUse` → `callus hook --cli claude --event pre`
  - `PostToolUse` → `callus hook --cli claude --event post`
  - `PostToolUseFailure` → `callus hook --cli claude --event failure`
  - `Stop` → `callus hook --cli claude --event stop`
- **Hint injection:** Returns JSON on stdout with `hookSpecificOutput.additionalContext` when a high-confidence fix matches on `PostToolUseFailure`.
- **Pre-tool guard:** Returns block/warning decision JSON only for user-enabled deterministic `PreToolUse` guard rules.
- **MCP:** Registers `callus mcp` as a stdio MCP server (or Streamable HTTP URL when pointing directly at a central server).

### 2.2 OpenAI Codex CLI
- **Events wired** (in `config.toml` under `[[hooks.PreToolUse]]` and `[[hooks.PostToolUse]]`, with `[features] hooks = true`):
  - `PreToolUse` → `callus hook --cli codex --event pre`
  - `PostToolUse` → `callus hook --cli codex --event post`
- **Failure detection:** Because Codex routes both successes and failures through `PostToolUse`, the `codex` adapter inspects exit status / error fields in the stdin payload and classifies `post_ok` vs. `post_failure` internally.
- **Pre-tool guard:** Emits `{"decision": "block", "reason": "..."}` on stdout for enabled guard rules.
- **Trust note:** `callus init --codex` reminds the user to approve project hooks via `codex /hooks` when installing into a repo-local `.codex/config.toml`.
- **MCP:** Adds `[mcp_servers.callus]` (`command = "callus"`, `args = ["mcp"]`) to `config.toml`.

### 2.3 Antigravity (`agy` / Antigravity IDE)
- **Events wired** (in `hooks.json` under top-level `"callus"` key, or packaged as a native `plugins/callus/plugin.json` bundle):
  - `PreToolUse` (`matcher: "*"`) → `callus hook --cli antigravity --event pre`
  - `PostToolUse` (`matcher: "*"`) → `callus hook --cli antigravity --event post`
  - `Stop` → `callus hook --cli antigravity --event stop`
- **Payload normalization:** Handles Antigravity's **camelCase** protojson fields (`conversationId`, `modelName`, `toolCall.name`, `toolCall.args`, `stepIdx`, `cwd`).
- **Failure detection:** Classifies `post_ok` vs. `post_failure` from step status/output fields in `PostToolUse`.
- **MCP:** Registers `callus` in `mcp_config.json`.

### 2.4 OpenCode
- **Plugin bridge:** `callus init --opencode` generates a lightweight (~30-line) TypeScript plugin at `~/.config/opencode/plugins/callus.ts` and enables it in `opencode.json`.
- **Events wired:**
  - `tool.execute.before` → calls `callus hook --cli opencode --event pre` (or posts to local `callus` HTTP socket if running)
  - `tool.execute.after` → inspects `output` for errors/exit codes and calls `callus hook --cli opencode --event post` (appending matched fix hints cleanly when a failure matches)
- **MCP:** Registers `"callus": { "type": "local", "command": ["callus", "mcp"] }` under `"mcp"` in `opencode.json`.

---

## 3. MCP Tools Exposed by `callus mcp`

Keep the MCP tool surface small and crystal clear (no tool-overload confusion):

1. `callus_search` — Hybrid vector + BM25 + graph lookup for a given error message, command, or task context (`os`, `shell`, `cli`, `model`, `repo`).
2. `callus_report` — Explicitly record a programming/workflow mistake and its verified fix when hooks could not observe it (e.g., logical bug, architectural pitfall).
3. `callus_feedback` — Mark whether a suggested fix worked or failed (supplements automatic closed-loop feedback).
4. `callus_export_candidates` — Returns top recurring patterns, counts, and stale rules so the user's own coding agent can draft tailored `AGENTS.md` / `CLAUDE.md` updates with zero extra server LLM cost.

---

## 4. `callus init` Safety Guarantees

- **Idempotent & non-destructive:** Always parses existing JSON/TOML configs cleanly and merges only the `callus` entries; never clobbers user hooks or other MCP servers.
- **Backup before edit:** Writes a timestamped `.bak` copy before modifying any CLI config file.
- **`--dry-run` & `--uninstall`:** Users can preview exact diffs before applying and cleanly remove every hook/plugin/MCP entry at any time.
- **Fail-open hooks:** If `callus hook` encounters any internal error or timeout, it exits cleanly without blocking the user's coding agent.
