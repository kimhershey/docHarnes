import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import type { RelationGraph } from "@/lib/types";

const W = 720;
const H = 420;
const R = 150;

/** 관계 그래프 section of the 결과 tab: documents on a circle, shared entities as labelled edges (§3.15). */
export function RelationsSection({ graph }: { graph: RelationGraph }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const n = graph.nodes.length;
  const positions = new Map(graph.nodes.map((node, i) => {
    const angle = (2 * Math.PI * i) / Math.max(1, n) - Math.PI / 2;
    return [node.doc_id, { x: W / 2 + R * Math.cos(angle), y: H / 2 + R * Math.sin(angle) }];
  }));
  if (n === 0 || graph.edges.length === 0) {
    return (
      <div className="flex flex-col gap-3" data-testid="project-relations">
        <p className="text-sm text-fg-muted" data-testid="project-relations-empty">
          {t("projects.relations.empty")}
        </p>
        {n > 0 && (
          <ul className="flex flex-wrap gap-2 text-xs">
            {graph.nodes.map((node) => (
              <li key={node.doc_id} className="rounded-full border border-border px-2 py-0.5">
                {node.label}
              </li>
            ))}
          </ul>
        )}
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-2" data-testid="project-relations">
      <svg viewBox={`0 0 ${W} ${H}`} className="w-full max-w-4xl rounded-lg border border-border bg-bg-elevated" role="img" aria-label={t("projects.relations.caption")}>
        <title>{t("projects.relations.caption")}</title>
        {graph.edges.map((e, i) => {
          const a = positions.get(e.from);
          const b = positions.get(e.to);
          if (!a || !b) return null;
          const mx = (a.x + b.x) / 2;
          const my = (a.y + b.y) / 2 + (i % 3) * 12 - 12;
          return (
            <g key={i} data-testid="relation-edge" data-kind={e.kind}>
              <line x1={a.x} y1={a.y} x2={b.x} y2={b.y} stroke="var(--border-strong)" strokeWidth={1.5} />
              <text x={mx} y={my} textAnchor="middle" fontSize={11} fill="var(--fg-muted)" className="select-none">
                {t(`projects.relations.kinds.${e.kind}`, { defaultValue: e.kind })} = {e.value}
              </text>
            </g>
          );
        })}
        {graph.nodes.map((node) => {
          const p = positions.get(node.doc_id)!;
          return (
            <g key={node.doc_id} className="cursor-pointer" onClick={() => navigate(`/library/${node.doc_id}`)} role="link" tabIndex={0} aria-label={t("projects.relations.open_doc", { name: node.label })} onKeyDown={(e) => e.key === "Enter" && navigate(`/library/${node.doc_id}`)} data-testid={`relation-node-${node.doc_id}`}>
              <circle cx={p.x} cy={p.y} r={26} fill="var(--accent-soft)" stroke="var(--accent)" strokeWidth={1.5} />
              <text x={p.x} y={p.y + 44} textAnchor="middle" fontSize={12} fill="var(--fg)" className="select-none">
                {node.label.length > 22 ? `${node.label.slice(0, 20)}…` : node.label}
              </text>
            </g>
          );
        })}
      </svg>
      <ul className="flex flex-col gap-0.5 text-xs text-fg-muted" data-testid="relation-list">
        {graph.edges.map((e, i) => {
          const from = graph.nodes.find((x) => x.doc_id === e.from)?.label ?? e.from;
          const to = graph.nodes.find((x) => x.doc_id === e.to)?.label ?? e.to;
          return (
            <li key={i} className="selectable">
              {from} ↔ {to}: {t(`projects.relations.kinds.${e.kind}`, { defaultValue: e.kind })} = {e.value}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
