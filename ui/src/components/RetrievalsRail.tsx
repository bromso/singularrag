import type { RetrievalSummary } from "@/api/types";
import { cn } from "@/lib/utils";

function rel(ms: number): string {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.round(s / 60)}m ago` : `${Math.round(s / 3600)}h ago`;
}
export function RetrievalsRail({ retrievals, selected, onSelect, onMore }: {
  retrievals: RetrievalSummary[]; selected: number | null; onSelect: (id: number) => void; onMore: () => void;
}) {
  const groups = new Map<string, RetrievalSummary[]>();
  for (const r of retrievals) groups.set(r.session_label, [...(groups.get(r.session_label) ?? []), r]);
  return (
    <section aria-label="Retrievals" className="flex h-full flex-col overflow-y-auto border-r">
      <h2 className="px-3 py-2 text-sm font-semibold">Retrievals</h2>
      {retrievals.length === 0 && (
        <div className="px-3 py-2 text-sm text-muted-foreground">
          <p>No retrievals yet. Connect an agent:</p>
          <pre className="mt-2 rounded bg-muted p-2 text-xs">claude mcp add --transport stdio singularrag -- singularrag mcp</pre>
        </div>
      )}
      {[...groups].map(([label, rs]) => (
        <div key={label}>
          <h3 className="px-3 pt-2 text-xs font-medium text-muted-foreground">{label}</h3>
          <ul>
            {rs.map((r) => (
              <li key={r.id}>
                <button type="button" aria-current={selected === r.id ? "true" : undefined} onClick={() => onSelect(r.id)}
                  className={cn("w-full px-3 py-2 text-left text-sm hover:bg-accent focus-visible:bg-accent", selected === r.id && "bg-accent")}>
                  <span className="block">{r.tool} · {r.query ?? "no query"}</span>
                  <span className="block text-xs text-muted-foreground">{rel(r.created_at_ms)} · {r.served} served · {r.cut} cut · {r.stale_count ? `${r.stale_count} stale` : "fresh"}</span>
                </button>
              </li>
            ))}
          </ul>
        </div>
      ))}
      {retrievals.length >= 50 && <button type="button" className="m-3 rounded border px-3 py-1 text-sm" onClick={onMore}>Older</button>}
    </section>
  );
}
