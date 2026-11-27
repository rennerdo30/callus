import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Check, Copy, Download } from "lucide-react";
import { api, type ExportFormat, type ExportScope } from "../api";

const FORMATS: { id: ExportFormat; label: string; filename: string }[] = [
  { id: "agents-md", label: "AGENTS.md", filename: "AGENTS.callus.md" },
  { id: "claude-md", label: "CLAUDE.md", filename: "CLAUDE.callus.md" },
  { id: "gemini-md", label: "GEMINI.md", filename: "GEMINI.callus.md" },
  { id: "hooks", label: "PreToolUse Guards", filename: "callus-guards.json" },
  { id: "pack", label: "Rule Pack JSON", filename: "callus-pack.json" },
];

const SCOPES: { id: ExportScope; label: string }[] = [
  { id: "all", label: "All Rules" },
  { id: "global", label: "Global (OS / CLI)" },
  { id: "repo", label: "Repo-Scoped" },
];

export function ExportView() {
  const [format, setFormat] = useState<ExportFormat>("agents-md");
  const [scope, setScope] = useState<ExportScope>("all");
  const [budgetLines, setBudgetLines] = useState(40);
  const [repoHash, setRepoHash] = useState("");
  const [copied, setCopied] = useState(false);

  const exportQuery = useQuery({
    queryKey: ["export", format, scope, budgetLines, repoHash],
    queryFn: () =>
      api.exportRules({
        format,
        scope,
        budget_lines: budgetLines,
        repo_hash: repoHash.trim() || undefined,
      }),
  });

  const content = exportQuery.data?.content ?? "";
  const lines = content ? content.split("\n") : [];
  const activeFormat = FORMATS.find((f) => f.id === format) ?? FORMATS[0];

  const handleCopy = async () => {
    if (!content) return;
    try {
      await navigator.clipboard.writeText(content);
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch {
      // Clipboard may be blocked in insecure contexts
    }
  };

  const handleDownload = () => {
    if (!content) return;
    const blob = new Blob([content], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = activeFormat.filename;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div className="space-y-4">
      {/* Control strip */}
      <div className="flex flex-col gap-3 rounded-lg border bg-surface p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex flex-wrap items-center gap-1">
            {FORMATS.map((f) => (
              <button
                key={f.id}
                type="button"
                onClick={() => setFormat(f.id)}
                className={`rounded-md px-3 py-1.5 font-mono text-xs font-medium transition ${
                  format === f.id
                    ? "bg-sky-500/15 text-sky-400 ring-1 ring-sky-500/30"
                    : "text-muted hover:bg-elevated hover:text-ink"
                }`}
              >
                {f.label}
              </button>
            ))}
          </div>

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={handleCopy}
              disabled={!content}
              className="inline-flex items-center gap-1.5 rounded-md border bg-elevated px-3 py-1.5 text-xs font-medium text-ink hover:border-sky-500/50 hover:text-sky-400 disabled:opacity-50"
            >
              {copied ? (
                <>
                  <Check className="h-3.5 w-3.5 text-emerald-400" />
                  <span>Copied</span>
                </>
              ) : (
                <>
                  <Copy className="h-3.5 w-3.5" />
                  <span>Copy</span>
                </>
              )}
            </button>
            <button
              type="button"
              onClick={handleDownload}
              disabled={!content}
              className="inline-flex items-center gap-1.5 rounded-md bg-sky-500 px-3 py-1.5 text-xs font-medium text-slate-950 hover:bg-sky-400 disabled:opacity-50"
            >
              <Download className="h-3.5 w-3.5" />
              <span>Download</span>
            </button>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-4 border-t pt-3 text-xs">
          <div className="flex items-center gap-1 rounded-md border bg-elevated p-0.5">
            {SCOPES.map((s) => (
              <button
                key={s.id}
                type="button"
                onClick={() => setScope(s.id)}
                className={`rounded px-2.5 py-1 text-xs font-medium transition ${
                  scope === s.id
                    ? "bg-surface text-ink shadow-xs"
                    : "text-muted hover:text-ink"
                }`}
              >
                {s.label}
              </button>
            ))}
          </div>

          <label className="flex items-center gap-2 text-muted">
            <span className="whitespace-nowrap">Line budget</span>
            <input
              type="range"
              min={10}
              max={120}
              step={5}
              value={budgetLines}
              onChange={(e) => setBudgetLines(Number(e.target.value))}
              className="w-28 accent-sky-400"
            />
            <span className="whitespace-nowrap font-mono text-ink tabular-nums">
              {budgetLines} lines
            </span>
          </label>

          {scope === "repo" && (
            <input
              type="text"
              value={repoHash}
              onChange={(e) => setRepoHash(e.target.value)}
              placeholder="Filter repo_hash (optional)"
              className="rounded-md border bg-canvas px-2.5 py-1 font-mono text-xs text-ink outline-none focus:border-sky-500"
            />
          )}

          <div className="ml-auto font-mono text-[11px] text-muted tabular-nums">
            {exportQuery.data?.candidate_count ?? 0} ranked candidate(s) ·{" "}
            {lines.length} lines rendered
          </div>
        </div>
      </div>

      {/* Live Preview Panel */}
      <section className="overflow-hidden rounded-lg border bg-surface">
        <div className="flex items-center justify-between border-b bg-elevated/60 px-4 py-2 font-mono text-xs text-muted">
          <span>{activeFormat.filename}</span>
          <span>Zero AGENTS.md bloat · ranked by frequency × Wilson</span>
        </div>
        {exportQuery.isLoading ? (
          <div className="p-6 text-xs text-muted">Generating preview…</div>
        ) : (
          <div className="max-h-[560px] overflow-auto bg-canvas p-4 font-mono text-xs leading-relaxed [font-variant-ligatures:none]">
            {lines.map((line, i) => {
              const isHunk = line.startsWith("@@");
              const isHeader =
                line.startsWith("--- ") ||
                line.startsWith("+++ ") ||
                line.startsWith("#");
              const isPrune =
                !isHeader &&
                (line.startsWith("-") ||
                  line.includes("Stale") ||
                  line.includes("Prune"));
              const isAddition = !isHeader && line.startsWith("+");
              const rowClass = isPrune
                ? "bg-rose-500/12 text-rose-300"
                : isAddition
                  ? "bg-emerald-500/12 text-emerald-300"
                  : isHunk
                    ? "bg-sky-500/10 text-sky-400 font-semibold"
                    : isHeader
                      ? "text-muted font-semibold"
                      : "text-ink";
              return (
                <div key={i} className={`flex gap-3 rounded px-1.5 ${rowClass}`}>
                  <span className="w-7 shrink-0 select-none text-right text-muted/60 tabular-nums">
                    {i + 1}
                  </span>
                  <span className="whitespace-pre-wrap break-words">
                    {line || " "}
                  </span>
                </div>
              );
            })}
          </div>
        )}
      </section>
    </div>
  );
}
