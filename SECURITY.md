# Security Policy

## Threat Model & Built-in Protections

Callus handles developer shell commands, stderr logs, and coding-agent hooks across local workstations and self-hosted team servers. The following security boundaries are enforced in code and covered by automated regression tests:

1. **Mandatory Client-Side Privacy Scrubbing (`callus-core/src/scrubber.rs`)**
   - Runs **before** local SQLite writes, MCP tool replies, rule pack imports/exports, and HTTP sync pushes.
   - Redacts provider API keys (AWS, GitHub/GitLab PATs, OpenAI, Anthropic, Google), JWTs, Bearer/Basic auth headers, URL credentials, PEM private keys, `.env` and `*_PASS`/`*_PWD` assignments, XML `<password>` elements, high-entropy tokens near secret keywords, emails, private IPs, internal hostnames, and Windows/Unix home & repository paths (including Claude encoded user slugs).
   - Custom regex deny-lists can be layered on top via `ScrubberConfig::deny_regexes` and can never weaken built-in rules.

2. **Strict Scope & Repository Isolation**
   - `personal` patterns and fixes stay strictly in local SQLite and are rejected both client-side (`callus sync`) and server-side (`POST /api/v1/sync/push`).
   - Short-retention `episodes` (raw scrubbed command snippets) are never synced to the central server and are pruned after 7 days.
   - `repo`-scoped patterns and `PreToolUse` guards only match when `query.repo_hash == pattern.repo_hash`.

3. **Prompt-Injection Defenses on Hook Output (`callus-cli/src/adapters.rs`)**
   - Every `PostToolUseFailure` `additionalContext` hint and `PreToolUse` guard reason is sanitized before being returned to the calling coding CLI:
     - Strips ANSI escape sequences, control characters, and Unicode bidi/zero-width spoofing characters (`U+200B..U+200F`, `U+202A..U+202E`, `U+2066..U+2069`, `U+FEFF`).
     - Collapses newlines into single spaces to prevent forged message/role boundaries.
     - Neutralizes prompt-override directives (`<system>`, `IGNORE PREVIOUS INSTRUCTIONS`, `SYSTEM OVERRIDE`, etc.) and caps length to 360 characters.

4. **Server Authentication & Minimal Attack Surface**
   - `callus serve` protects `/api/v1/*` and `/mcp` with constant-time Bearer token verification (`CALLUS_API_TOKEN`).
   - The production Docker image runs as a non-root system user (`callus`, UID `10001`) on `debian:bookworm-slim` with zero Python/PyTorch/GPU runtimes.

## Reporting a Vulnerability

Please do **not** open a public GitHub issue for security vulnerabilities. Report vulnerabilities privately via GitHub Security Advisories on `rennerdo30/callus` with:
- A minimal reproduction input or payload
- Affected version (`callus --version`)
- Expected vs actual behavior
