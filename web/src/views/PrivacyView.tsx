import { useEffect, useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { Database, Lock, Play, ShieldCheck, Sparkles } from "lucide-react";
import { api, type HealthResponse, type OverviewResponse } from "../api";

interface PrivacyViewProps {
  health: HealthResponse | undefined;
  overview: OverviewResponse | undefined;
}

const SAMPLE_STDERR = `error: failed to connect to https://deploy:s3cr3tP4ss!@10.24.18.42:5432/prod_db
AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
GITHUB_TOKEN=ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8
Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U
at C:\\Users\\alice\\repos\\payments\\src\\main.rs:42:9
contact devops@internal.corp.local for DB_PASS=hunter2secret`;

function renderHighlightedScrubbed(text: string) {
  const parts = text.split(/(<REDACTED:[A-Z0-9_]+>|<HOME>|<REPO>)/g);
  return parts.map((part, idx) => {
    if (part.startsWith("<REDACTED:")) {
      return (
        <span
          key={idx}
          className="inline-block whitespace-nowrap rounded bg-rose-500/20 px-1 py-0.5 text-rose-300 ring-1 ring-rose-500/30"
        >
          {part}
        </span>
      );
    }
    if (part === "<HOME>" || part === "<REPO>") {
      return (
        <span
          key={idx}
          className="inline-block whitespace-nowrap rounded bg-sky-500/20 px-1 py-0.5 text-sky-300 ring-1 ring-sky-500/30"
        >
          {part}
        </span>
      );
    }
    return <span key={idx}>{part}</span>;
  });
}

export function PrivacyView({ health, overview }: PrivacyViewProps) {
  const [input, setInput] = useState(SAMPLE_STDERR);
  const scrub = useMutation({
    mutationFn: (text: string) => api.scrubTest(text),
  });

  useEffect(() => {
    scrub.mutate(SAMPLE_STDERR);
    // Run once on initial view open for 1-second verification
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const redactions = [...(scrub.data?.redactions ?? [])].sort(
    (a, b) => b.count - a.count,
  );
  const totalHits = redactions.reduce((acc, item) => acc + item.count, 0);

  return (
    <div className="grid gap-4 lg:grid-cols-12">
      {/* Live Client-Side Privacy Scrubber Playground */}
      <section className="min-w-0 space-y-3 rounded-lg border bg-surface p-4 lg:col-span-8">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="min-w-0">
            <h2 className="flex items-center gap-1.5 text-sm font-semibold tracking-tight">
              <ShieldCheck className="h-4 w-4 text-emerald-400" />
              <span>Privacy Scrubber Playground</span>
            </h2>
            <p className="text-xs text-muted">
              Every hook event and sync payload passes through this exact scrubber
              before SQLite write or HTTP sync.
            </p>
          </div>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => {
                setInput(SAMPLE_STDERR);
                scrub.mutate(SAMPLE_STDERR);
              }}
              className="rounded-md border bg-elevated px-2.5 py-1.5 text-xs text-muted hover:text-ink"
            >
              Load Sample
            </button>
            <button
              type="button"
              onClick={() => scrub.mutate(input)}
              disabled={scrub.isPending || !input.trim()}
              className="inline-flex items-center gap-1.5 rounded-md bg-sky-500 px-3 py-1.5 text-xs font-medium text-slate-950 hover:bg-sky-400 disabled:opacity-50"
            >
              <Play className="h-3.5 w-3.5" />
              <span>Test Redaction</span>
            </button>
          </div>
        </div>

        <textarea
          rows={6}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="Paste a messy stderr, stack trace, or command containing secrets/paths..."
          className="w-full rounded-md border bg-canvas p-3 font-mono text-xs text-ink outline-none focus:border-sky-500"
        />

        {scrub.data && (
          <div className="space-y-2.5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="text-xs font-semibold text-emerald-400 tabular-nums">
                Scrubbed Output ({totalHits} redaction{totalHits === 1 ? "" : "s"})
              </span>
              <div className="flex flex-wrap items-center gap-1.5">
                {redactions.map((item) => (
                  <span
                    key={item.category}
                    className="rounded bg-rose-500/15 px-2 py-0.5 font-mono text-[11px] text-rose-300 ring-1 ring-rose-500/30 tabular-nums"
                  >
                    {item.category}: {item.count}
                  </span>
                ))}
              </div>
            </div>
            <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded-md border bg-canvas p-3 font-mono text-xs leading-relaxed text-ink">
              {renderHighlightedScrubbed(scrub.data.scrubbed)}
            </pre>
          </div>
        )}
      </section>

      {/* Diagnostics & External llama.cpp Status */}
      <aside className="min-w-0 space-y-4 lg:col-span-4">
        <section className="rounded-lg border bg-surface p-4">
          <h3 className="flex items-center gap-1.5 text-sm font-semibold tracking-tight">
            <Database className="h-4 w-4 text-sky-400" />
            <span>Runtime &amp; Storage</span>
          </h3>
          <dl className="mt-3 space-y-2 text-xs">
            <div className="flex items-center justify-between border-b pb-2">
              <dt className="text-muted">Backend</dt>
              <dd className="font-mono font-medium uppercase">
                {health?.backend ?? "—"}
              </dd>
            </div>
            <div className="flex items-center justify-between border-b pb-2">
              <dt className="text-muted">Version</dt>
              <dd className="font-mono">v{health?.version ?? "—"}</dd>
            </div>
            <div className="flex items-center justify-between border-b pb-2">
              <dt className="text-muted">Bearer Auth</dt>
              <dd className="inline-flex items-center gap-1 font-mono">
                <Lock className="h-3 w-3 text-muted" />
                {health?.auth_enabled ? "Protected" : "Local open"}
              </dd>
            </div>
            <div className="flex items-center justify-between">
              <dt className="text-muted">Sync Boundary</dt>
              <dd className="font-mono text-emerald-400">
                personal never synced
              </dd>
            </div>
          </dl>
        </section>

        <section className="rounded-lg border bg-surface p-4">
          <h3 className="flex items-center gap-1.5 text-sm font-semibold tracking-tight">
            <Sparkles className="h-4 w-4 text-amber-400" />
            <span>External llama.cpp Embeddings</span>
          </h3>
          <p className="mt-1 text-xs text-muted">
            Zero local GPU or PyTorch runtime. Callus connects over HTTP to any
            OpenAI-compatible{" "}
            <code className="font-mono text-ink">/v1/embeddings</code> endpoint
            and falls back cleanly to BM25 + Graph when offline.
          </p>
          <div className="mt-3 rounded-md border bg-elevated p-2.5 text-xs">
            <div className="flex items-center justify-between">
              <span className="text-muted">Status</span>
              <span
                className={`font-mono font-medium ${
                  health?.embedding_enabled
                    ? "text-emerald-400"
                    : "text-amber-400"
                }`}
              >
                {health?.embedding_enabled
                  ? `Active (${health.embedding_dims}d)`
                  : "Offline BM25 + Graph"}
              </span>
            </div>
            <div className="mt-1.5 flex items-center justify-between">
              <span className="text-muted">Pending Backfill</span>
              <span className="font-mono tabular-nums">
                {overview?.pending_embeddings ?? 0} pattern(s)
              </span>
            </div>
          </div>
          <pre className="mt-3 overflow-x-auto rounded-md border bg-canvas p-2.5 font-mono text-[11px] text-muted">
{`EMBEDDING_PROVIDER=openai-compatible
EMBEDDING_URL=http://llama-host:8081/v1/embeddings
EMBEDDING_DIMS=${health?.embedding_dims ?? 384}`}
          </pre>
        </section>
      </aside>
    </div>
  );
}
