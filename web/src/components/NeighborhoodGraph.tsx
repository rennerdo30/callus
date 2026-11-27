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
    220,
    Math.max(leftNodes.length, rightNodes.length, 1) * 52 + 44,
  );
  const centerX = 340;
  const centerY = height / 2;

  const yPos = (idx: number, count: number) => {
    if (count <= 1) return centerY;
    const span = height - 76;
    return 38 + (idx * span) / (count - 1);
  };

  return (
    <div className="space-y-2.5">
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
            const lx = 212;
            const ly = (ny + centerY) / 2;
            const label =
              item.edge.hit_count > 0
                ? `${item.edge.hit_count}×`
                : item.edge.edge_type;
            return (
              <g key={`le-${item.node.id}`}>
                <path
                  d={`M 188 ${ny} C 228 ${ny}, 232 ${centerY}, 256 ${centerY}`}
                  fill="none"
                  stroke={active ? "#38bdf8" : "#334155"}
                  strokeWidth={active ? 2 : 1.3}
                  strokeDasharray={item.hop === 2 ? "4 3" : undefined}
                />
                <rect
                  x={lx - 13}
                  y={ly - 7}
                  width="26"
                  height="14"
                  rx="4"
                  fill="#0b0e14"
                />
                <text
                  x={lx}
                  y={ly + 3}
                  textAnchor="middle"
                  className="fill-slate-300 font-mono text-[9px]"
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
                  d={`M 424 ${centerY} C 446 ${centerY}, 448 ${ny}, 492 ${ny}`}
                  fill="none"
                  stroke={active ? "#10b981" : good ? "#10b98188" : "#f43f5e88"}
                  strokeWidth={active ? 2.2 : 1.5}
                />
                <rect
                  x={rx - 22}
                  y={ry - 7}
                  width="44"
                  height="14"
                  rx="4"
                  fill="#0b0e14"
                />
                <text
                  x={rx}
                  y={ry + 3}
                  textAnchor="middle"
                  className="fill-slate-200 font-mono text-[9px]"
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
                className="cursor-pointer"
              >
                <rect
                  x="14"
                  y={ny - 18}
                  width="174"
                  height="36"
                  rx="6"
                  fill={active ? "#1e293b" : "#121722"}
                  stroke={active ? "#38bdf8" : "#243044"}
                  strokeWidth={active ? 1.8 : 1}
                />
                <text
                  x="24"
                  y={ny - 4}
                  className="fill-slate-400 font-mono text-[9px] uppercase"
                >
                  {item.node.kind} · hop {item.hop}
                </text>
                <text
                  x="24"
                  y={ny + 10}
                  className="fill-slate-100 font-mono text-[11px]"
                >
                  {truncate(item.node.label, 22)}
                </text>
              </g>
            );
          })}

          {/* Center ErrorPattern focal node */}
          <g>
            <rect
              x={centerX - 84}
              y={centerY - 26}
              width="168"
              height="52"
              rx="8"
              fill="#0f172a"
              stroke="#38bdf8"
              strokeWidth="2"
            />
            <text
              x={centerX}
              y={centerY - 7}
              textAnchor="middle"
              className="fill-sky-400 font-mono text-[9px] uppercase"
            >
              ERROR PATTERN
            </text>
            <text
              x={centerX}
              y={centerY + 10}
              textAnchor="middle"
              className="fill-slate-100 text-[11px] font-medium"
            >
              {truncate(rootLabel, 21)}
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
                className="cursor-pointer"
              >
                <rect
                  x="492"
                  y={ny - 18}
                  width="174"
                  height="36"
                  rx="6"
                  fill={active ? "#1e293b" : "#121722"}
                  stroke={active ? "#38bdf8" : strokeColor}
                  strokeWidth={active ? 2 : 1.4}
                />
                <text
                  x="502"
                  y={ny - 4}
                  className="fill-slate-400 font-mono text-[9px] uppercase"
                >
                  {item.node.kind} · {status}
                </text>
                <text
                  x="502"
                  y={ny + 10}
                  className="fill-slate-100 font-mono text-[11px]"
                >
                  {truncate(displayLabel, 22)}
                </text>
              </g>
            );
          })}
        </svg>
      </div>

      {selected && (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border bg-elevated px-3 py-2 text-xs">
          <div className="flex flex-wrap items-center gap-2 font-mono">
            <span className="rounded bg-surface px-1.5 py-0.5 text-[10px] uppercase text-sky-400">
              {selected.node.kind}
            </span>
            <span className="text-ink">{selected.node.label}</span>
            <span className="text-muted">
              ({selected.edge.edge_type}, hop {selected.hop})
            </span>
          </div>
          <div className="font-mono text-[11px] text-muted tabular-nums">
            worked {selected.edge.worked_count} · failed{" "}
            {selected.edge.failed_count} · hits {selected.edge.hit_count}
          </div>
        </div>
      )}
    </div>
  );
}
