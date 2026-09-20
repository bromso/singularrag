import type { Item, TreeFile, TreeSymbol } from "@/api/types";
import type { ItemStatus } from "./status";

export type SymbolRow = { key: string; symbol: TreeSymbol; status: ItemStatus; item: Item | null };
export type FileRow = { path: string; lang: string | null; served: number; cut: number; expandedByDefault: boolean; symbols: SymbolRow[] };

export function joinRetrieval(files: TreeFile[], items: Item[] | null): FileRow[] {
  const byKey = new Map<string, Item>();
  for (const it of items ?? []) byKey.set(`${it.path}::${it.name}::${it.line_start}`, it);
  const rows = files.map<FileRow>((f) => {
    let served = 0, cut = 0;
    const symbols = f.symbols.map<SymbolRow>((s) => {
      const key = `${f.path}::${s.name}::${s.line_start}`;
      const item = byKey.get(key) ?? null;
      const status: ItemStatus = item ? (item.served ? "served" : "cut") : "untouched";
      if (item) item.served ? served++ : cut++;
      return { key, symbol: s, status, item };
    });
    return { path: f.path, lang: f.lang, served, cut, expandedByDefault: served + cut > 0, symbols };
  });
  if (items) rows.sort((a, b) => Number(b.expandedByDefault) - Number(a.expandedByDefault) || a.path.localeCompare(b.path));
  return rows;
}
