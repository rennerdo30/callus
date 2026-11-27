import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, Search, XCircle } from "lucide-react";
import { api, type FixStatus, type Scope } from "../api";
import {
  FixTypeBadge,
  ScopeBadge,
  StatusBadge,
  formatShortTime,
} from "../components/badges";
import { NeighborhoodGraph } from "../components/NeighborhoodGraph";

interface PatternsViewProps {
  selectedPatternId: string | null;
  onSelectPattern: (id: string) => void;
}

const SCOPE_FILTERS: { id: string; label: string }[] = [
  { id: "", label: "All Scopes" },
  { id: "team", label: "Team" },
  { id: "repo", label: "Repo" },
  { id: "personal", label: "Personal" },
];

export function PatternsView({
  selectedPatternId,
  onSelectPattern,
}: PatternsViewProps) {
  const [scopeFilter, setScopeFilter] = useState("");
  const [searchText, setSearchText] = useState("");
  const [submittedQuery, setSubmittedQuery] = useState("");
  const queryClient = useQueryClient();

  const patternsQuery = useQuery({
    queryKey: ["patterns", scopeFilter],
    queryFn: () =>
      api.listPatterns({ scope: scopeFilter || undefined, limit: 100 }),
  });

  const searchQuery = useQuery({
    queryKey: ["search", submittedQuery],
    queryFn: () => api.search(submittedQuery, 15),
    enabled: submittedQuery.trim().length > 0,
  });

  const patterns = patternsQuery.data?.patterns ?? [];

  useEffect(() => {
    if (!selectedPatternId && patterns.length > 0) {
      onSelectPattern(patterns[0].id);
    }
  }, [selectedPatternId, patterns, onSelectPattern]);

  const activeId = selectedPatternId ?? patterns[0]?.id ?? null;

  const detailQuery = useQuery({
    queryKey: ["pattern-detail", activeId],
    queryFn: () => api.patternDetail(activeId!),
    enabled: Boolean(activeId),
  });

  const graphQuery = useQuery({
    queryKey: ["graph", activeId],
    queryFn: () => api.graph(`err:${activeId}`, 2),
    enabled: Boolean(activeId),
  });

  const scopeMutation = useMutation({
    mutationFn: ({
      id,
      scope,
      repo_hash,
    }: {
      id: string;
      scope: Scope;
      repo_hash: string | null;
    }) => api.updatePatternScope(id, scope, repo_hash),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["patterns"] });
      void queryClient.invalidateQueries({ queryKey: ["pattern-detail"] });
      void queryClient.invalidateQueries({ queryKey: ["overview"] });
    },
  });

  const fixMutation = useMutation({
    mutationFn: ({ id, status }: { id: string; status: FixStatus }) =>
      api.updateFixStatus(id, status),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["pattern-detail"] });
      void queryClient.invalidateQueries({ queryKey: ["graph"] });
      void queryClient.invalidateQueries({ queryKey: ["overview"] });
    },
  });

  const handleSearch = (e: React.FormEvent) => {
    e.preventDefault();
    setSubmittedQuery(searchText.trim());
  };

  return (
    <div className="space-y-4">
      {/* Instant Hybrid Search + Scope Filters */}
      <div className="flex flex-col gap-3 rounded-lg border bg-surface p-3.5 sm:flex-row sm:items-center sm:justify-between">
        <form onSubmit={handleSearch} className="flex flex-1 items-center gap-2">
          <div className="relative flex-1">
            <Search className="pointer-events-none absolute top-1/2 left-3 h-3.5 w-3.5 -translate-y-1/2 text-muted" />
            <input
              type="text"
              value={searchText}
              onChange={(e) => {
                setSearchText(e.target.value);
                if (!e.target.value.trim()) setSubmittedQuery("");
              }}
              placeholder="Hybrid search error stderr, command, or fix..."
              className="w-full rounded-md border bg-canvas py-1.5 pr-3 pl-8 font-mono text-xs text-ink outline-none focus:border-sky-500"
            />
          </div>
          <button
            type="submit"
            className="shrink-0 rounded-md bg-sky-500 px-3 py-1.5 text-xs font-medium text-slate-950 hover:bg-sky-400"
          >
            Search
          </button>
          {submittedQuery && (
            <button
              type="button"
              onClick={() => {
                setSearchText("");
                setSubmittedQuery("");
              }}
              className="shrink-0 rounded-md border bg-elevated px-2.5 py-1.5 text-xs text-muted hover:text-ink"
            >
              Clear
            </button>
          )}
        </form>

        <div className="flex flex-wrap items-center gap-1">
          {SCOPE_FILTERS.map((f) => (
            <button
              key={f.id || "all"}
              type="button"
              onClick={() => {
                setScopeFilter(f.id);
                setSubmittedQuery("");
              }}
              className={`rounded-md px-2.5 py-1 text-xs font-medium transition ${
                scopeFilter === f.id && !submittedQuery
                  ? "bg-sky-500/15 text-sky-400 ring-1 ring-sky-500/30"
                  : "text-muted hover:bg-elevated hover:text-ink"
              }`}
            >
              {f.label}
            </button>
          ))}
        </div>
      </div>

      {/* Hybrid Search Result Strip (when searching) */}
      {submittedQuery && searchQuery.data && (
        <div className="rounded-lg border border-sky-500/30 bg-sky-500/5 p-3">
          <div className="mb-2 flex items-center justify-between text-xs">
            <span className="font-medium text-sky-400">
              {searchQuery.data.hits.length} hybrid match(es) for &ldquo;
              {submittedQuery}&rdquo;
            </span>
            <span className="font-mono text-[11px] text-muted">
              {searchQuery.data.vector_used
                ? "RRF(Vector + BM25) × Graph × Wilson"
                : "BM25 + Signature × Graph × Wilson"}
            </span>
          </div>
          {searchQuery.data.hits.length === 0 ? (
            <p className="text-xs text-muted">No matching fixes found.</p>
          ) : (
            <div className="grid gap-2 sm:grid-cols-2">
              {searchQuery.data.hits.slice(0, 4).map((hit) => (
                <button
                  key={`${hit.pattern.id}:${hit.fix.id}`}
                  type="button"
                  onClick={() => onSelectPattern(hit.pattern.id)}
                  className="rounded-md border bg-surface p-2.5 text-left hover:border-sky-500/50"
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate text-xs font-medium text-ink">
                      {hit.pattern.title}
                    </span>
                    <span className="shrink-0 font-mono text-[11px] text-emerald-400 tabular-nums">
                      score {hit.score.toFixed(2)}
                    </span>
                  </div>
                  <p className="mt-1 truncate text-xs text-muted">
                    {hit.fix.summary}
                  </p>
                </button>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Split Master-Detail Workspace */}
      <div className="grid gap-4 lg:grid-cols-12">
        {/* Pattern Cluster List */}
        <aside className="min-w-0 rounded-lg border bg-surface lg:col-span-4">
          <div className="flex items-center justify-between border-b px-3.5 py-2.5 text-xs">
            <span className="font-semibold">Error Clusters</span>
            <span className="font-mono text-muted tabular-nums">
              {patterns.length}
            </span>
          </div>
          <div className="max-h-[640px] divide-y overflow-y-auto">
            {patterns.length === 0 ? (
              <div className="p-4 text-xs text-muted">
                No patterns match this filter.
              </div>
            ) : (
              patterns.map((pat) => {
                const selected = pat.id === activeId;
                return (
                  <button
                    key={pat.id}
                    type="button"
                    onClick={() => onSelectPattern(pat.id)}
                    className={`w-full px-3.5 py-2.5 text-left transition ${
                      selected
                        ? "bg-sky-500/10 ring-1 ring-inset ring-sky-500/30"
                        : "hover:bg-elevated/60"
                    }`}
                  >
                    <div className="flex items-start justify-between gap-2">
                      <span className="line-clamp-1 text-xs font-medium text-ink">
                        {pat.title}
                      </span>
                      <span className="shrink-0 font-mono text-xs font-semibold text-muted tabular-nums">
                        {pat.occurrence_count}×
                      </span>
                    </div>
                    <p className="mt-1 truncate font-mono text-[11px] text-muted">
                      {pat.error_template}
                    </p>
                    <div className="mt-1.5 flex items-center gap-1.5">
                      <ScopeBadge scope={pat.scope} />
                      <span className="font-mono text-[10px] text-muted">
                        {pat.tool_name}
                      </span>
                      <span className="ml-auto text-[10px] text-muted">
                        {formatShortTime(pat.last_seen_at)}
                      </span>
                    </div>
                  </button>
                );
              })
            )}
          </div>
        </aside>

        {/* Pattern Inspector + 2-hop Graph + Fixes */}
        <section className="min-w-0 space-y-4 lg:col-span-8">
          {!detailQuery.data ? (
            <div className="rounded-lg border bg-surface p-8 text-center text-xs text-muted">
              Select an error cluster to inspect its graph and candidate fixes.
            </div>
          ) : (
            <>
              {/* Pattern header & scope controls */}
              <div className="rounded-lg border bg-surface p-4">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <h2 className="text-base font-semibold tracking-tight">
                        {detailQuery.data.pattern.title}
                      </h2>
                      <ScopeBadge scope={detailQuery.data.pattern.scope} />
                    </div>
                    <div className="mt-1 flex flex-wrap items-center gap-3 font-mono text-[11px] text-muted">
                      <span>tool: {detailQuery.data.pattern.tool_name}</span>
                      <span>
                        sig: {detailQuery.data.pattern.signature_hash.slice(0, 12)}
                      </span>
                      <span className="tabular-nums">
                        {detailQuery.data.pattern.occurrence_count} occurrences
                      </span>
                      {detailQuery.data.pattern.repo_hash && (
                        <span>repo: {detailQuery.data.pattern.repo_hash}</span>
                      )}
                    </div>
                  </div>

                  {/* Scope switcher */}
                  <div className="flex items-center gap-1 rounded-md border bg-elevated p-0.5 text-[11px]">
                    {(["personal", "repo", "team"] as Scope[]).map((sc) => {
                      const active = detailQuery.data.pattern.scope === sc;
                      return (
                        <button
                          key={sc}
                          type="button"
                          disabled={scopeMutation.isPending}
                          onClick={() =>
                            scopeMutation.mutate({
                              id: detailQuery.data.pattern.id,
                              scope: sc,
                              repo_hash:
                                sc === "repo"
                                  ? (detailQuery.data.pattern.repo_hash ?? "local")
                                  : null,
                            })
                          }
                          className={`rounded px-2 py-1 font-mono transition ${
                            active
                              ? "bg-sky-500 text-slate-950 font-semibold"
                              : "text-muted hover:text-ink"
                          }`}
                        >
                          {sc}
                        </button>
                      );
                    })}
                  </div>
                </div>

                <pre className="mt-3 overflow-x-auto rounded-md border bg-canvas p-2.5 font-mono text-xs text-rose-300">
                  {detailQuery.data.pattern.error_template}
                </pre>
              </div>

              {/* 1-2 Hop Interactive Neighborhood Graph */}
              <div className="rounded-lg border bg-surface p-4">
                <div className="mb-2.5 flex items-center justify-between">
                  <div>
                    <h3 className="text-sm font-semibold tracking-tight">
                      1–2 Hop Knowledge Graph
                    </h3>
                    <p className="text-xs text-muted">
                      Contexts (OCCURS_IN) → ErrorPattern → Fixes (RESOLVED_BY)
                    </p>
                  </div>
                  <span className="font-mono text-[11px] text-muted tabular-nums">
                    {graphQuery.data?.neighbors.length ?? 0} linked nodes
                  </span>
                </div>
                <NeighborhoodGraph
                  rootLabel={detailQuery.data.pattern.title}
                  neighbors={graphQuery.data?.neighbors ?? []}
                  fixes={detailQuery.data.fixes}
                />
              </div>

              {/* Candidate & Verified Fixes with 1-click Verify / Deprecate */}
              <div className="rounded-lg border bg-surface">
                <div className="flex items-center justify-between border-b px-4 py-2.5">
                  <h3 className="text-sm font-semibold tracking-tight">
                    Linked Fixes &amp; Guards
                  </h3>
                  <span className="font-mono text-xs text-muted tabular-nums">
                    {detailQuery.data.fixes.length} fix(es)
                  </span>
                </div>
                {detailQuery.data.fixes.length === 0 ? (
                  <div className="p-4 text-xs text-muted">
                    No fixes linked yet. Callus pairs one automatically when an
                    agent recovers from this error.
                  </div>
                ) : (
                  <div className="divide-y">
                    {detailQuery.data.fixes.map(({ fix, edge }) => (
                      <div key={fix.id} className="space-y-2.5 p-4">
                        <div className="flex flex-wrap items-start justify-between gap-3">
                          <div className="min-w-0 flex-1 space-y-1">
                            <div className="flex flex-wrap items-center gap-2">
                              <StatusBadge status={fix.status} />
                              <FixTypeBadge fixType={fix.fix_type} />
                              <span className="font-mono text-[11px] text-emerald-400 tabular-nums">
                                {edge.worked_count} worked
                              </span>
                              <span className="font-mono text-[11px] text-rose-400 tabular-nums">
                                {edge.failed_count} failed
                              </span>
                            </div>
                            <p className="text-sm text-ink">{fix.summary}</p>
                          </div>

                          <div className="flex shrink-0 items-center gap-1.5">
                            {fix.status !== "verified" && (
                              <button
                                type="button"
                                disabled={fixMutation.isPending}
                                onClick={() =>
                                  fixMutation.mutate({
                                    id: fix.id,
                                    status: "verified",
                                  })
                                }
                                className="inline-flex items-center gap-1 rounded-md bg-emerald-500/15 px-2.5 py-1 text-xs font-medium text-emerald-400 ring-1 ring-emerald-500/30 hover:bg-emerald-500/25"
                              >
                                <CheckCircle2 className="h-3.5 w-3.5" />
                                <span>Verify</span>
                              </button>
                            )}
                            {fix.status !== "deprecated" && (
                              <button
                                type="button"
                                disabled={fixMutation.isPending}
                                onClick={() =>
                                  fixMutation.mutate({
                                    id: fix.id,
                                    status: "deprecated",
                                  })
                                }
                                className="inline-flex items-center gap-1 rounded-md bg-rose-500/15 px-2.5 py-1 text-xs font-medium text-rose-400 ring-1 ring-rose-500/30 hover:bg-rose-500/25"
                              >
                                <XCircle className="h-3.5 w-3.5" />
                                <span>Deprecate</span>
                              </button>
                            )}
                          </div>
                        </div>

                        {(fix.pattern_before || fix.pattern_after || fix.guard_regex) && (
                          <div className="grid gap-2 rounded-md border bg-canvas p-2.5 font-mono text-xs sm:grid-cols-2">
                            {fix.pattern_before && (
                              <div className="min-w-0">
                                <div className="text-[10px] uppercase text-rose-400">
                                  Avoid
                                </div>
                                <div className="truncate text-rose-300">
                                  - {fix.pattern_before}
                                </div>
                              </div>
                            )}
                            {fix.pattern_after && (
                              <div className="min-w-0">
                                <div className="text-[10px] uppercase text-emerald-400">
                                  Use instead
                                </div>
                                <div className="truncate text-emerald-300">
                                  + {fix.pattern_after}
                                </div>
                              </div>
                            )}
                            {fix.guard_regex && (
                              <div className="min-w-0 sm:col-span-2">
                                <div className="text-[10px] uppercase text-amber-400">
                                  PreToolUse Guard Regex
                                </div>
                                <div className="truncate text-amber-300">
                                  /{fix.guard_regex}/
                                </div>
                              </div>
                            )}
                          </div>
                        )}
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </>
          )}
        </section>
      </div>
    </div>
  );
}
