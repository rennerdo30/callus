import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  ArrowRight,
  Cpu,
  Flame,
  RefreshCw,
  ShieldAlert,
  Sparkles,
} from "lucide-react";
import { api, type OverviewResponse } from "../api";
import { ScopeBadge, formatShortTime } from "../components/badges";

interface OverviewViewProps {
  data: OverviewResponse | undefined;
  isLoading: boolean;
  onSelectPattern: (patternId: string) => void;
}

const KIND_LABELS: Record<string, string> = {
  environment: "OS / Shell",
  cli: "CLI",
  model: "Model",
  repo: "Repo",
};

const KIND_COLORS: Record<string, string> = {
  environment: "bg-sky-500",
  cli: "bg-emerald-500",
  model: "bg-violet-500",
  repo: "bg-amber-500",
};

export function OverviewView({
  data,
  isLoading,
  onSelectPattern,
}: OverviewViewProps) {
  const queryClient = useQueryClient();
  const consolidate = useMutation({
    mutationFn: api.consolidate,
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["overview"] });
      void queryClient.invalidateQueries({ queryKey: ["patterns"] });
    },
  });

  if (isLoading || !data) {
    return (
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        {[1, 2, 3, 4].map((n) => (
          <div
            key={n}
            className="h-24 animate-pulse rounded-lg border bg-surface"
          />
        ))}
      </div>
    );
  }

  const maxCtxHits = Math.max(1, ...data.context_breakdown.map((c) => c.hits));
  const totalScoped =
    data.scope_counts.team +
    data.scope_counts.repo +
    data.scope_counts.personal +
    data.scope_counts.public_pack;

  return (
    <div className="space-y-5">
      {/* 4 single-glance KPI tiles */}
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <div className="rounded-lg border bg-surface p-4">
          <div className="flex items-center justify-between text-xs text-muted">
            <span>Retries Saved</span>
            <Sparkles className="h-4 w-4 text-emerald-400" />
          </div>
          <div className="mt-1.5 flex items-baseline gap-2">
            <span className="text-2xl font-semibold tracking-tight text-emerald-400 tabular-nums">
              {data.retries_saved.toLocaleString()}
            </span>
            <span className="text-xs text-muted">verified hits</span>
          </div>
          <div className="mt-1.5 font-mono text-[11px] text-muted tabular-nums">
            {data.verified_fix_count} verified · {data.candidate_fix_count} candidate
          </div>
        </div>

        <div className="rounded-lg border bg-surface p-4">
          <div className="flex items-center justify-between text-xs text-muted">
            <span>Recurring Patterns</span>
            <Flame className="h-4 w-4 text-rose-400" />
          </div>
          <div className="mt-1.5 flex items-baseline gap-2">
            <span className="text-2xl font-semibold tracking-tight tabular-nums">
              {data.pattern_count.toLocaleString()}
            </span>
            <span className="text-xs text-muted tabular-nums">
              {data.total_occurrences.toLocaleString()} total hits
            </span>
          </div>
          <div className="mt-1.5 font-mono text-[11px] text-muted tabular-nums">
            {data.scope_counts.team} team · {data.scope_counts.repo} repo ·{" "}
            {data.scope_counts.personal} personal
          </div>
        </div>

        <div className="rounded-lg border bg-surface p-4">
          <div className="flex items-center justify-between text-xs text-muted">
            <span>Active Pre-Tool Guards</span>
            <ShieldAlert className="h-4 w-4 text-amber-400" />
          </div>
          <div className="mt-1.5 flex items-baseline gap-2">
            <span className="text-2xl font-semibold tracking-tight text-amber-400 tabular-nums">
              {data.active_guard_count}
            </span>
            <span className="text-xs text-muted">blocking rules</span>
          </div>
          <div className="mt-1.5 font-mono text-[11px] text-muted tabular-nums">
            {data.obsolete_fix_count} deprecated / obsolete
          </div>
        </div>

        <div className="rounded-lg border bg-surface p-4">
          <div className="flex items-center justify-between text-xs text-muted">
            <span>Retrieval Engine</span>
            <Cpu className="h-4 w-4 text-sky-400" />
          </div>
          <div className="mt-1.5 flex items-center justify-between gap-2">
            <span className="text-sm font-semibold tracking-tight">
              {data.embedding_enabled
                ? `Hybrid Vector (${data.embedding_dims}d)`
                : "BM25 + Graph"}
            </span>
            <button
              type="button"
              onClick={() => consolidate.mutate()}
              disabled={consolidate.isPending}
              className="inline-flex items-center gap-1 rounded border bg-elevated px-2 py-1 text-[11px] font-medium text-ink transition hover:border-sky-500/50 hover:text-sky-400 disabled:opacity-50"
            >
              <RefreshCw
                className={`h-3 w-3 ${consolidate.isPending ? "animate-spin" : ""}`}
              />
              <span>Consolidate</span>
            </button>
          </div>
          <div className="mt-1.5 font-mono text-[11px] text-muted tabular-nums">
            {consolidate.data ? (
              <span className="text-emerald-400">
                +{consolidate.data.backfilled_embeddings} emb · +
                {consolidate.data.promoted_patterns} promoted ·{" "}
                {consolidate.data.obsoleted_fixes} obsoleted
              </span>
            ) : (
              `${data.pending_embeddings} pending embeddings · ${data.backend}`
            )}
          </div>
        </div>
      </div>

      {/* Main 2-column content */}
      <div className="grid gap-4 lg:grid-cols-3">
        {/* Hotspots table */}
        <section className="min-w-0 rounded-lg border bg-surface lg:col-span-2">
          <div className="flex items-center justify-between border-b px-4 py-3">
            <div className="min-w-0">
              <h2 className="text-sm font-semibold tracking-tight">
                Top Recurring Failure Hotspots
              </h2>
              <p className="text-xs text-muted">
                Click any cluster to inspect its 2-hop graph and verify fixes
              </p>
            </div>
            <span className="shrink-0 font-mono text-xs text-muted tabular-nums">
              top {data.hotspots.length}
            </span>
          </div>

          {data.hotspots.length === 0 ? (
            <div className="p-6 text-center text-xs text-muted">
              No recurring patterns recorded yet. Run{" "}
              <code className="rounded bg-elevated px-1.5 py-0.5 font-mono text-ink">
                callus init
              </code>{" "}
              to capture failures automatically across Claude Code, Codex,
              Antigravity, and OpenCode.
            </div>
          ) : (
            <div className="divide-y">
              {data.hotspots.map((pat) => (
                <button
                  key={pat.id}
                  type="button"
                  onClick={() => onSelectPattern(pat.id)}
                  className="group flex w-full items-start justify-between gap-3 px-4 py-3 text-left transition hover:bg-elevated/60"
                >
                  <div className="min-w-0 flex-1 space-y-1">
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="min-w-0 truncate text-sm font-medium text-ink group-hover:text-sky-400">
                        {pat.title}
                      </span>
                      <span className="shrink-0">
                        <ScopeBadge scope={pat.scope} />
                      </span>
                      <span className="shrink-0 rounded bg-elevated px-1.5 py-0.5 font-mono text-[10px] text-muted">
                        {pat.tool_name}
                      </span>
                    </div>
                    <p className="truncate font-mono text-xs text-muted">
                      {pat.error_template !== pat.title
                        ? pat.error_template
                        : `sig:${pat.signature_hash.slice(0, 12)} · ${pat.embedding_status} embedding`}
                    </p>
                  </div>
                  <div className="flex shrink-0 items-center gap-2.5 pt-0.5">
                    <div className="text-right">
                      <div className="font-mono text-sm font-semibold text-ink tabular-nums">
                        {pat.occurrence_count}×
                      </div>
                      <div className="text-[11px] text-muted">
                        {formatShortTime(pat.last_seen_at)}
                      </div>
                    </div>
                    <ArrowRight className="h-4 w-4 text-muted transition group-hover:translate-x-0.5 group-hover:text-sky-400" />
                  </div>
                </button>
              ))}
            </div>
          )}
        </section>

        {/* Right rail: Context breakdown + Scope mix */}
        <div className="min-w-0 space-y-4">
          <section className="rounded-lg border bg-surface p-4">
            <h2 className="text-sm font-semibold tracking-tight">
              Environment &amp; CLI Breakdown
            </h2>
            <p className="mt-0.5 text-xs text-muted">
              Where recurring failures happen most
            </p>

            {data.context_breakdown.length === 0 ? (
              <p className="mt-4 text-xs text-muted">
                No context edges recorded yet.
              </p>
            ) : (
              <div className="mt-3.5 space-y-2.5">
                {data.context_breakdown.slice(0, 10).map((ctx) => {
                  const pct = Math.max(6, Math.round((ctx.hits / maxCtxHits) * 100));
                  return (
                    <div key={`${ctx.kind}:${ctx.label}`} className="space-y-1">
                      <div className="flex items-center justify-between text-xs">
                        <div className="flex min-w-0 items-center gap-1.5">
                          <span className="rounded bg-elevated px-1 py-0.5 font-mono text-[10px] text-muted">
                            {KIND_LABELS[ctx.kind] ?? ctx.kind}
                          </span>
                          <span className="truncate font-mono text-ink">
                            {ctx.label}
                          </span>
                        </div>
                        <span className="ml-2 shrink-0 font-mono text-xs text-muted tabular-nums">
                          {ctx.hits}
                        </span>
                      </div>
                      <div className="h-1.5 w-full overflow-hidden rounded-full bg-elevated">
                        <div
                          className={`h-full rounded-full ${KIND_COLORS[ctx.kind] ?? "bg-sky-500"}`}
                          style={{ width: `${pct}%` }}
                        />
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </section>

          <section className="rounded-lg border bg-surface p-4">
            <h2 className="text-sm font-semibold tracking-tight">
              Repository &amp; Scope Isolation
            </h2>
            <p className="mt-0.5 text-xs text-muted">
              Generic patterns auto-promote to team at ≥ 2 repos
            </p>
            <div className="mt-3 flex h-2.5 w-full overflow-hidden rounded-full bg-elevated">
              {totalScoped > 0 && (
                <>
                  <div
                    className="bg-sky-500"
                    style={{
                      width: `${(data.scope_counts.team / totalScoped) * 100}%`,
                    }}
                    title={`Team: ${data.scope_counts.team}`}
                  />
                  <div
                    className="bg-amber-500"
                    style={{
                      width: `${(data.scope_counts.repo / totalScoped) * 100}%`,
                    }}
                    title={`Repo: ${data.scope_counts.repo}`}
                  />
                  <div
                    className="bg-slate-400"
                    style={{
                      width: `${(data.scope_counts.personal / totalScoped) * 100}%`,
                    }}
                    title={`Personal: ${data.scope_counts.personal}`}
                  />
                  <div
                    className="bg-violet-500"
                    style={{
                      width: `${(data.scope_counts.public_pack / totalScoped) * 100}%`,
                    }}
                    title={`Public Pack: ${data.scope_counts.public_pack}`}
                  />
                </>
              )}
            </div>
            <div className="mt-3 grid grid-cols-2 gap-2 text-xs">
              <div className="flex items-center justify-between rounded bg-elevated px-2.5 py-1.5">
                <span className="flex items-center gap-1.5 text-muted">
                  <span className="h-2 w-2 rounded-full bg-sky-500" />
                  Team
                </span>
                <span className="font-mono font-medium tabular-nums">
                  {data.scope_counts.team}
                </span>
              </div>
              <div className="flex items-center justify-between rounded bg-elevated px-2.5 py-1.5">
                <span className="flex items-center gap-1.5 text-muted">
                  <span className="h-2 w-2 rounded-full bg-amber-500" />
                  Repo
                </span>
                <span className="font-mono font-medium tabular-nums">
                  {data.scope_counts.repo}
                </span>
              </div>
              <div className="flex items-center justify-between rounded bg-elevated px-2.5 py-1.5">
                <span className="flex items-center gap-1.5 text-muted">
                  <span className="h-2 w-2 rounded-full bg-slate-400" />
                  Personal
                </span>
                <span className="font-mono font-medium tabular-nums">
                  {data.scope_counts.personal}
                </span>
              </div>
              <div className="flex items-center justify-between rounded bg-elevated px-2.5 py-1.5">
                <span className="flex items-center gap-1.5 text-muted">
                  <span className="h-2 w-2 rounded-full bg-violet-500" />
                  Pack
                </span>
                <span className="font-mono font-medium tabular-nums">
                  {data.scope_counts.public_pack}
                </span>
              </div>
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
