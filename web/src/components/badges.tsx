import type { FixStatus, FixType, Scope } from "../api";

export function ScopeBadge({ scope }: { scope: Scope }) {
  const styles: Record<Scope, string> = {
    team: "bg-sky-500/15 text-sky-400 ring-sky-500/30",
    repo: "bg-amber-500/15 text-amber-400 ring-amber-500/30",
    personal: "bg-slate-500/15 text-muted ring-slate-500/30",
    public_pack: "bg-violet-500/15 text-violet-400 ring-violet-500/30",
  };
  const labels: Record<Scope, string> = {
    team: "team",
    repo: "repo",
    personal: "personal",
    public_pack: "pack",
  };
  return (
    <span
      className={`inline-flex items-center rounded px-1.5 py-0.5 font-mono text-[11px] font-medium ring-1 ${styles[scope]}`}
    >
      {labels[scope]}
    </span>
  );
}

export function StatusBadge({ status }: { status: FixStatus }) {
  const styles: Record<FixStatus, string> = {
    verified: "bg-emerald-500/15 text-emerald-400 ring-emerald-500/30",
    candidate: "bg-amber-500/15 text-amber-400 ring-amber-500/30",
    deprecated: "bg-rose-500/15 text-rose-400 ring-rose-500/30",
    obsolete: "bg-slate-500/15 text-muted ring-slate-500/30 line-through",
  };
  return (
    <span
      className={`inline-flex items-center rounded px-1.5 py-0.5 font-mono text-[11px] font-medium ring-1 ${styles[status]}`}
    >
      {status}
    </span>
  );
}

export function FixTypeBadge({ fixType }: { fixType: FixType }) {
  const labels: Record<FixType, string> = {
    hint: "live hint",
    pre_tool_guard: "pre-tool guard",
    rule_snippet: "rule snippet",
    workflow: "workflow",
  };
  return (
    <span className="inline-flex items-center rounded bg-elevated px-1.5 py-0.5 font-mono text-[11px] text-muted ring-1 ring-line">
      {labels[fixType]}
    </span>
  );
}

export function formatShortTime(iso: string): string {
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return iso.slice(0, 10);
  const diffSec = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (diffSec < 60) return `${diffSec}s ago`;
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin}m ago`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr}h ago`;
  const diffDay = Math.floor(diffHr / 24);
  if (diffDay < 60) return `${diffDay}d ago`;
  return iso.slice(0, 10);
}
