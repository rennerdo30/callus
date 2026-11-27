import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Activity,
  FileCode2,
  KeyRound,
  Moon,
  Network,
  ShieldCheck,
  Sun,
} from "lucide-react";
import { ApiHttpError, api, getStoredToken, setStoredToken } from "./api";
import { OverviewView } from "./views/OverviewView";
import { PatternsView } from "./views/PatternsView";
import { ExportView } from "./views/ExportView";
import { PrivacyView } from "./views/PrivacyView";

export type TabId = "overview" | "patterns" | "export" | "privacy";

const TABS: { id: TabId; label: string; icon: typeof Activity }[] = [
  { id: "overview", label: "Overview", icon: Activity },
  { id: "patterns", label: "Patterns & Graph", icon: Network },
  { id: "export", label: "Export Studio", icon: FileCode2 },
  { id: "privacy", label: "Scrubber & Settings", icon: ShieldCheck },
];

export function App() {
  const [activeTab, setActiveTab] = useState<TabId>("overview");
  const [selectedPatternId, setSelectedPatternId] = useState<string | null>(null);
  const [dark, setDark] = useState<boolean>(() => {
    try {
      return localStorage.getItem("callus_theme") !== "light";
    } catch {
      return true;
    }
  });
  const [tokenInput, setTokenInput] = useState<string>(() => getStoredToken());
  const [authPromptOpen, setAuthPromptOpen] = useState(false);
  const queryClient = useQueryClient();

  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    try {
      localStorage.setItem("callus_theme", dark ? "dark" : "light");
    } catch {
      // Ignore storage errors
    }
  }, [dark]);

  const health = useQuery({
    queryKey: ["healthz"],
    queryFn: api.health,
  });

  const overview = useQuery({
    queryKey: ["overview"],
    queryFn: api.overview,
    enabled: !health.data?.auth_enabled || Boolean(getStoredToken()),
  });

  const needsAuth =
    (health.data?.auth_enabled && !getStoredToken()) ||
    (overview.error instanceof ApiHttpError && overview.error.status === 401);

  const handleSaveToken = (e: React.FormEvent) => {
    e.preventDefault();
    setStoredToken(tokenInput);
    setAuthPromptOpen(false);
    void queryClient.invalidateQueries();
  };

  const handleJumpToPattern = (id: string) => {
    setSelectedPatternId(id);
    setActiveTab("patterns");
  };

  return (
    <div className="min-h-screen overflow-x-clip bg-canvas text-ink">
      <header className="sticky top-0 z-30 border-b bg-surface/95 backdrop-blur">
        <div className="mx-auto flex max-w-7xl flex-wrap items-center justify-between gap-3 px-5 py-3">
          <div className="flex items-center gap-3">
            <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-sky-500/15 text-sky-400 ring-1 ring-sky-500/30">
              <ShieldCheck className="h-4.5 w-4.5" />
            </div>
            <div className="leading-tight">
              <div className="flex items-center gap-2 text-base font-semibold tracking-tight">
                <span>Callus</span>
                {health.data && (
                  <span className="rounded bg-elevated px-2 py-0.5 font-mono text-xs text-muted">
                    v{health.data.version}
                  </span>
                )}
              </div>
              <p className="hidden text-xs text-muted sm:block">
                Mistake memory &amp; guard compiler
              </p>
            </div>
          </div>

          <nav
            aria-label="Primary views"
            className="order-3 flex w-full flex-wrap items-center gap-1.5 md:order-2 md:w-auto"
          >
            {TABS.map((tab) => {
              const Icon = tab.icon;
              const selected = activeTab === tab.id;
              return (
                <button
                  key={tab.id}
                  type="button"
                  onClick={() => setActiveTab(tab.id)}
                  className={`flex shrink-0 cursor-pointer items-center gap-2 rounded-lg px-3 py-2 text-sm font-medium transition ${
                    selected
                      ? "bg-sky-500/15 text-sky-400 ring-1 ring-sky-500/30"
                      : "text-muted hover:bg-elevated hover:text-ink"
                  }`}
                >
                  <Icon className="h-4 w-4" />
                  <span>{tab.label}</span>
                </button>
              );
            })}
          </nav>

          <div className="order-2 flex items-center gap-2 md:order-3">
            {health.data && (
              <div
                className="hidden items-center gap-2 rounded-full border bg-elevated px-3 py-1.5 text-xs sm:flex"
                title={
                  health.data.embedding_enabled
                    ? `${health.data.embedding_provider === "local" ? "Local" : "Remote"} ${health.data.embedding_model ?? "embeddings"} (${health.data.embedding_dims}d)`
                    : "Lexical BM25 + Knowledge Graph"
                }
              >
                <span className="h-2 w-2 rounded-full bg-emerald-400" />
                <span className="font-mono font-medium uppercase">
                  {health.data.backend}
                </span>
                <span className="text-muted">·</span>
                <span className="text-muted">
                  {health.data.embedding_enabled
                    ? health.data.embedding_provider === "local"
                      ? `Local ${health.data.embedding_dims}d`
                      : `Remote ${health.data.embedding_dims}d`
                    : "BM25 + Graph"}
                </span>
              </div>
            )}
            {health.data?.auth_enabled && (
              <button
                type="button"
                onClick={() => setAuthPromptOpen((v) => !v)}
                className="flex cursor-pointer items-center gap-1.5 rounded-lg border bg-elevated px-2.5 py-2 text-xs font-medium text-muted transition hover:border-sky-500/40 hover:text-ink"
                title="Configure API Bearer token"
              >
                <KeyRound className="h-4 w-4" />
                <span className="hidden sm:inline">Token</span>
              </button>
            )}
            <button
              type="button"
              onClick={() => setDark((d) => !d)}
              className="cursor-pointer rounded-lg border bg-elevated p-2 text-muted transition hover:border-sky-500/40 hover:text-ink"
              aria-label="Toggle color theme"
              title={dark ? "Switch to light mode" : "Switch to dark mode"}
            >
              {dark ? (
                <Sun className="h-4 w-4" />
              ) : (
                <Moon className="h-4 w-4" />
              )}
            </button>
          </div>
        </div>
      </header>

      <main className="mx-auto max-w-7xl px-5 py-6">
        {(needsAuth || authPromptOpen) && (
          <form
            onSubmit={handleSaveToken}
            className="mb-6 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-amber-500/30 bg-amber-500/10 p-4"
          >
            <div className="text-sm">
              <div className="font-semibold text-amber-300">
                Bearer API token required
              </div>
              <div className="text-muted">
                Stored only in this browser tab&apos;s sessionStorage and sent via
                Authorization header.
              </div>
            </div>
            <div className="flex flex-1 items-center gap-2 sm:max-w-md">
              <input
                type="password"
                value={tokenInput}
                onChange={(e) => setTokenInput(e.target.value)}
                placeholder="Paste CALLUS_API_TOKEN..."
                className="w-full rounded-md border bg-surface px-3 py-2 font-mono text-sm text-ink outline-none focus:border-sky-500"
              />
              <button
                type="submit"
                className="shrink-0 cursor-pointer rounded-md bg-sky-500 px-3.5 py-2 text-sm font-medium text-slate-950 transition hover:bg-sky-400"
              >
                Unlock
              </button>
            </div>
          </form>
        )}

        {activeTab === "overview" && (
          <OverviewView
            data={overview.data}
            isLoading={overview.isLoading}
            onSelectPattern={handleJumpToPattern}
          />
        )}
        {activeTab === "patterns" && (
          <PatternsView
            selectedPatternId={selectedPatternId}
            onSelectPattern={setSelectedPatternId}
          />
        )}
        {activeTab === "export" && <ExportView />}
        {activeTab === "privacy" && (
          <PrivacyView health={health.data} overview={overview.data} />
        )}
      </main>
    </div>
  );
}
