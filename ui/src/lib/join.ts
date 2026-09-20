import type { Item, TreeFile, TreeSymbol } from "@/api/types";
import type { ItemStatus } from "./status";

export type SymbolRow = { key: string; symbol: TreeSymbol; status: ItemStatus; item: Item | null; moved: boolean };
export type FileRow = { path: string; lang: string | null; served: number; cut: number; expandedByDefault: boolean; symbols: SymbolRow[] };

type Hit = { item: Item; moved: boolean };

/** Match items to symbols by id, then by exact position, then by name at another line (moved). */
function assign(files: TreeFile[], items: Item[]): Map<number, Hit> {
  const byId = new Map<number, { path: string; symbol: TreeSymbol }>();
  const byPathName = new Map<string, TreeSymbol[]>();
  for (const f of files) {
    for (const s of f.symbols) {
      byId.set(s.id, { path: f.path, symbol: s });
      const k = `${f.path}::${s.name}`;
      const list = byPathName.get(k);
      if (list) list.push(s); else byPathName.set(k, [s]);
    }
  }
  const hits = new Map<number, Hit>();
  const taken = new Set<number>();
  for (const it of [...items].sort((a, b) => a.rank - b.rank)) {
    const idHit = byId.get(it.symbol_id);
    if (idHit && idHit.path === it.path && idHit.symbol.name === it.name && !taken.has(idHit.symbol.id)) {
      hits.set(idHit.symbol.id, { item: it, moved: false });
      taken.add(idHit.symbol.id);
      continue;
    }
    const candidates = (byPathName.get(`${it.path}::${it.name}`) ?? []).filter((s) => !taken.has(s.id));
    if (candidates.length === 0) continue;
    const exact = candidates.find((s) => s.line_start === it.line_start);
    const chosen = exact ?? candidates.reduce((best, s) => (Math.abs(s.line_start - it.line_start) < Math.abs(best.line_start - it.line_start) ? s : best)); // ties keep the earlier symbol: array order is file order
    hits.set(chosen.id, { item: it, moved: !exact });
    taken.add(chosen.id);
  }
  return hits;
}

export function joinRetrieval(files: TreeFile[], items: Item[] | null): FileRow[] {
  const hits = items ? assign(files, items) : new Map<number, Hit>();
  const rows = files.map<FileRow>((f) => {
    let served = 0, cut = 0;
    const symbols = f.symbols.map<SymbolRow>((s) => {
      const hit = hits.get(s.id) ?? null;
      const item = hit?.item ?? null;
      const status: ItemStatus = item ? (item.served ? "served" : "cut") : "untouched";
      if (item) item.served ? served++ : cut++;
      return { key: `${f.path}::${s.name}::${s.line_start}`, symbol: s, status, item, moved: hit?.moved ?? false };
    });
    return { path: f.path, lang: f.lang, served, cut, expandedByDefault: served + cut > 0, symbols };
  });
  if (items) rows.sort((a, b) => Number(b.expandedByDefault) - Number(a.expandedByDefault) || a.path.localeCompare(b.path));
  return rows;
}
