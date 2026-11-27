export type Scope = "personal" | "repo" | "team" | "public_pack";
export type FixStatus = "candidate" | "verified" | "deprecated" | "obsolete";
export type FixType = "hint" | "pre_tool_guard" | "rule_snippet" | "workflow";
export type ExportFormat = "agents-md" | "claude-md" | "gemini-md" | "hooks" | "pack";
export type ExportScope = "repo" | "global" | "all";

export interface HealthResponse {
  status: string;
  backend: "postgres" | "sqlite" | string;
  embedding_enabled: boolean;
  embedding_dims: number;
  auth_enabled: boolean;
  version: string;
}

export interface ErrorPattern {
  id: string;
  signature_hash: string;
  title: string;
  error_template: string;
  tool_name: string;
  scope: Scope;
  repo_hash: string | null;
  occurrence_count: number;
  first_seen_at: string;
  last_seen_at: string;
  embedding_status: "pending" | "ready" | "skipped";
  updated_at: string;
}

export interface Fix {
  id: string;
  summary: string;
  fix_type: FixType;
  pattern_before: string | null;
  pattern_after: string | null;
  guard_regex: string | null;
  status: FixStatus;
  scope: Scope;
  source: string;
  created_at: string;
  updated_at: string;
}

export interface GraphNode {
  id: string;
  kind: "error_pattern" | "fix" | "environment" | "cli" | "model" | "repo" | "tool" | "rule";
  label: string;
  props_json: Record<string, unknown>;
}

export interface GraphEdge {
  src_id: string;
  edge_type: string;
  dst_id: string;
  worked_count: number;
  failed_count: number;
  hit_count: number;
  last_seen_at: string;
}

export interface FixCandidate {
  fix: Fix;
  edge: GraphEdge;
}

export interface Neighbor {
  node: GraphNode;
  edge: GraphEdge;
  direction: "outgoing" | "incoming";
  hop: number;
}

export interface ContextStat {
  kind: string;
  label: string;
  hits: number;
}

export interface OverviewResponse {
  pattern_count: number;
  total_occurrences: number;
  retries_saved: number;
  active_guard_count: number;
  verified_fix_count: number;
  candidate_fix_count: number;
  obsolete_fix_count: number;
  pending_embeddings: number;
  embedding_enabled: boolean;
  embedding_dims: number;
  backend: string;
  version: string;
  scope_counts: Record<Scope, number>;
  context_breakdown: ContextStat[];
  hotspots: ErrorPattern[];
}

export interface PatternDetailResponse {
  pattern: ErrorPattern;
  fixes: FixCandidate[];
  contexts: GraphNode[];
}

export interface GraphResponse {
  root: string;
  neighbors: Neighbor[];
}

export interface SearchHit {
  pattern: ErrorPattern;
  fix: Fix;
  edge: GraphEdge;
  score: number;
  wilson: number;
  exact_signature: boolean;
}

export interface SearchResponse {
  hits: SearchHit[];
  vector_used: boolean;
}

export interface ExportResponse {
  content: string;
  candidate_count: number;
}

export interface ScrubReport {
  scrubbed: string;
  hits: Record<string, number>;
}

export interface ConsolidationReport {
  backfilled_embeddings: number;
  promoted_patterns: number;
  obsoleted_fixes: number;
  pruned_episodes: number;
}

const TOKEN_STORAGE_KEY = "callus_api_token";

export function getStoredToken(): string {
  try {
    return sessionStorage.getItem(TOKEN_STORAGE_KEY) ?? "";
  } catch {
    return "";
  }
}

export function setStoredToken(token: string): void {
  try {
    const trimmed = token.trim();
    if (trimmed) {
      sessionStorage.setItem(TOKEN_STORAGE_KEY, trimmed);
    } else {
      sessionStorage.removeItem(TOKEN_STORAGE_KEY);
    }
  } catch {
    // Ignore storage unavailability
  }
}

export class ApiHttpError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const token = getStoredToken();
  const headers = new Headers(init?.headers);
  if (init?.body && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }
  if (token) {
    headers.set("Authorization", `Bearer ${token}`);
  }
  const resp = await fetch(path, { ...init, headers });
  if (!resp.ok) {
    let message = resp.statusText;
    try {
      const payload = (await resp.json()) as { error?: string };
      if (payload.error) message = payload.error;
    } catch {
      // Fall back to statusText
    }
    throw new ApiHttpError(resp.status, message);
  }
  return (await resp.json()) as T;
}

export const api = {
  health: () => apiFetch<HealthResponse>("/healthz"),
  overview: () => apiFetch<OverviewResponse>("/api/v1/overview"),
  listPatterns: (params?: { scope?: string; repo_hash?: string; limit?: number }) => {
    const q = new URLSearchParams();
    if (params?.scope) q.set("scope", params.scope);
    if (params?.repo_hash) q.set("repo_hash", params.repo_hash);
    if (params?.limit) q.set("limit", String(params.limit));
    const qs = q.toString();
    return apiFetch<{ patterns: ErrorPattern[] }>(`/api/v1/patterns${qs ? `?${qs}` : ""}`);
  },
  patternDetail: (id: string) => apiFetch<PatternDetailResponse>(`/api/v1/patterns/${encodeURIComponent(id)}`),
  updatePatternScope: (id: string, scope: Scope, repo_hash: string | null) =>
    apiFetch<{ ok: boolean }>(`/api/v1/patterns/${encodeURIComponent(id)}/scope`, {
      method: "POST",
      body: JSON.stringify({ scope, repo_hash }),
    }),
  updateFixStatus: (id: string, status: FixStatus) =>
    apiFetch<{ ok: boolean }>(`/api/v1/fixes/${encodeURIComponent(id)}/status`, {
      method: "POST",
      body: JSON.stringify({ status }),
    }),
  graph: (nodeId: string, hops = 2) =>
    apiFetch<GraphResponse>(`/api/v1/graph/${encodeURIComponent(nodeId)}?hops=${hops}`),
  search: (text: string, limit = 15) =>
    apiFetch<SearchResponse>("/api/v1/search", {
      method: "POST",
      body: JSON.stringify({ text, tool_name: null, context: null, limit }),
    }),
  exportRules: (params: {
    format: ExportFormat;
    scope: ExportScope;
    budget_lines: number;
    repo_hash?: string;
    stale_days?: number;
  }) => {
    const q = new URLSearchParams({
      format: params.format,
      scope: params.scope,
      budget_lines: String(params.budget_lines),
    });
    if (params.repo_hash) q.set("repo_hash", params.repo_hash);
    if (params.stale_days) q.set("stale_days", String(params.stale_days));
    return apiFetch<ExportResponse>(`/api/v1/export?${q.toString()}`);
  },
  scrubTest: (text: string) =>
    apiFetch<ScrubReport>("/api/v1/scrub-test", {
      method: "POST",
      body: JSON.stringify({ text }),
    }),
  consolidate: () =>
    apiFetch<ConsolidationReport>("/api/v1/admin/consolidate", {
      method: "POST",
    }),
};
