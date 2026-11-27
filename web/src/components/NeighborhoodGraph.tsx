import { useState } from "react";
import type { FixCandidate, Neighbor } from "../api";

interface NeighborhoodGraphProps {
  rootLabel: string;
  neighbors: Neighbor[];
  fixes?: FixCandidate[];
}

function truncate(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

export function NeighborhoodGraph({
  rootLabel,
  neighbors,
  fixes = [],
}: NeighborhoodGraphProps) {
  const [selected, setSelected] = useState<Neighbor | null>(null);
  const fixByNodeId = new Map(fixes.map((fc) => [`fix:${fc.fix.id}`, fc.fix]));

  // Deduplicate by node.id, preferring hop 1
  const uniqueMap = new Map<string, Neighbor>();
  for (const n of neighbors) {
    const existing = uniqueMap.get(n.node.id);
    if (!existing || n.hop < existing.hop) {
      uniqueMap.set(n.node.id, n);
    }
  }
  const unique = Array.from(uniqueMap.values());
  const leftNodes = unique
    .filter((n) => n.node.kind !== "fix" && n.node.kind !== "rule")
    .slice(0, 6);
  const rightNodes = unique
    .filter((n) => n.node.kind === "fix" || n.node.kind === "rule")
    .slice(0, 6);

  const height = Math.max(
    240,
    Math.max(leftNodes.length, rightNodes.length, 1) * 60 + 52,
  );
  const centerX = 340;
  const centerY = height / 2;

  const yPos = (idx: number, count: number) => {
    if (count <= 1) return centerY;
    const span = height - 88;
    return 44 + (idx * span) / (count - 1);
  };

  return (
    <div className="space-y-3">
      <div className="overflow-hidden rounded-lg border bg-canvas">
        <svg
          viewBox={`0 0 680 ${height}`}
          className="h-auto w-full select-none"
          role="img"
          aria-label="1 to 2 hop error pattern knowledge graph"
        >
          {/* Left edges (Contexts -> ErrorPattern) */}
          {leftNodes.map((item, idx) => {
            const ny = yPos(idx, leftNodes.length);
            const active = selected?.node.id === item.node.id;
            const lx = 216;
            const ly = (ny + centerY) / 2;
            const label =
              item.edge.hit_count > 0
                ? `${item.edge.hit_count}×`
                : item.edge.edge_type;
            return (
              <g key={`le-${item.node.id}`}>
                <path
                  d={`M 190 ${ny} C 228 ${ny}, 232 ${centerY}, 252 ${centerY}`}
                  fill="none"
                  stroke={active ? "#38bdf8" : "#334155"}
                  strokeWidth={active ? 2 : 1.4}
                  strokeDasharray={item.hop === 2 ? "4 3" : undefined}
                />
                <rect
                  x={lx - 15}
                  y={ly - 8}
                  width="30"
                  height="16"
                  rx="4"
                  fill="#0b0e14"
                />
                <text
                  x={lx}
                  y={ly + 3.5}
                  textAnchor="middle"
                  className="fill-slate-300 font-mono text-[10px]"
                >
                  {label}
                </text>
              </g>
            );
          })}

          {/* Right edges (ErrorPattern -> Fixes) */}
          {rightNodes.map((item, idx) => {
            const ny = yPos(idx, rightNodes.length);
            const active = selected?.node.id === item.node.id;
            const good = item.edge.worked_count >= item.edge.failed_count;
            const rx = 458;
            const ry = (ny + centerY) / 2;
            return (
              <g key={`re-${item.node.id}`}>
                <path
                  d={`M 428 ${centerY} C 448 ${centerY}, 450 ${ny}, 490 ${ny}`}
                  fill="none"
                  stroke={active ? "#10b981" : good ? "#10b98188" : "#f43f5e88"}
                  strokeWidth={active ? 2.2 : 1.5}
                />
                <rect
                  x={rx - 25}
                  y={ry - 8}
                  width="50"
                  height="16"
                  rx="4"
                  fill="#0b0e14"
                />
                <text
                  x={rx}
                  y={ry + 3.5}
                  textAnchor="middle"
                  className="fill-slate-200 font-mono text-[10px]"
                >
                  {item.edge.worked_count}✓/{item.edge.failed_count}✗
                </text>
              </g>
            );
          })}

          {/* Left context nodes */}
          {leftNodes.map((item, idx) => {
            const ny = yPos(idx, leftNodes.length);
            const active = selected?.node.id === item.node.id;
            return (
              <g
                key={item.node.id}
                onClick={() => setSelected(item)}
                role="button"
                tabIndex={0}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    setSelected(item);
                  }
                }}
                className="group cursor-pointer outline-none"
              >
                <rect
                  x="12"
                  y={ny - 22}
                  width="178"
                  height="44"
                  rx="7"
                  fill={active ? "#1e293b" : "#121722"}
                  stroke={active ? "#38bdf8" : "#243044"}
                  strokeWidth={active ? 2 : 1.2}
                  className="transition-colors group-hover:fill-[#192234] group-hover:stroke-sky-400"
                />
                <text
                  x="22"
                  y={ny - 5}
                  className="fill-slate-400 font-mono text-[10px] uppercase"
                >
                  {item.node.kind} · hop {item.hop}
                </text>
                <text
                  x="22"
                  y={ny + 12}
                  className="fill-slate-100 font-mono text-[12px]"
                >
                  {truncate(item.node.label, 20)}
                </text>
              </g>
            );
          })}

          {/* Center ErrorPattern focal node */}
          <g>
            <rect
              x={centerX - 88}
              y={centerY - 29}
              width="176"
              height="58"
              rx="8"
              fill="#0f172a"
              stroke="#38bdf8"
              strokeWidth="2"
            />
            <text
              x={centerX}
              y={centerY - 8}
              textAnchor="middle"
              className="fill-sky-400 font-mono text-[10px] uppercase"
            >
              ERROR PATTERN
            </text>
            <text
              x={centerX}
              y={centerY + 12}
              textAnchor="middle"
              className="fill-slate-100 text-[12px] font-medium"
            >
              {truncate(rootLabel, 20)}
            </text>
          </g>

          {/* Right fix nodes */}
          {rightNodes.map((item, idx) => {
            const ny = yPos(idx, rightNodes.length);
            const active = selected?.node.id === item.node.id;
            const liveFix = fixByNodeId.get(item.node.id);
            const status =
              liveFix?.status ??
              (typeof item.node.props_json?.status === "string"
                ? item.node.props_json.status
                : "candidate");
            const displayLabel = liveFix?.summary ?? item.node.label;
            const strokeColor =
              status === "verified"
                ? "#10b981"
                : status === "deprecated" || status === "obsolete"
                  ? "#f43f5e"
                  : "#f59e0b";
            return (
              <g
                key={item.node.id}
                onClick={() => setSelected(item)}
                role="button"
                tabIndex={0}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    setSelected(item);
                  }
                }}
                className="group cursor-pointer outline-none"
              >
                <rect
                  x="490"
                  y={ny - 22}
                  width="178"
                  height="44"
                  rx="7"
                  fill={active ? "#1e293b" : "#121722"}
                  stroke={active ? "#38bdf8" : strokeColor}
                  strokeWidth={active ? 2 : 1.5}
                  className="transition-colors group-hover:fill-[#192234] group-hover:stroke-sky-400"
                />
                <text
                  x="500"
                  y={ny - 5}
                  className="fill-slate-400 font-mono text-[10px] uppercase"
                >
                  {item.node.kind} · {status}
                </text>
                <text
                  x="500"
                  y={ny + 12}
                  className="fill-slate-100 font-mono text-[12px]"
                >
                  {truncate(displayLabel, 20)}
                </text>
              </g>
            );
          })}
        </svg>
      </div>

      {selected && (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border bg-elevated px-3.5 py-2.5 text-sm">
          <div className="flex flex-wrap items-center gap-2 font-mono text-xs">
            <span className="rounded bg-surface px-2 py-0.5 text-xs uppercase text-sky-400">
              {selected.node.kind}
            </span>
            <span className="text-ink">{selected.node.label}</span>
            <span className="text-muted">
              ({selected.edge.edge_type}, hop {selected.hop})
            </span>
          </div>
          <div className="font-mono text-xs text-muted tabular-nums">
            worked {selected.edge.worked_count} · failed{" "}
            {selected.edge.failed_count} · hits {selected.edge.hit_count}
          </div>
        </div>
      )}
    </div>
  );
}
