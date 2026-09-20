import { useEffect, useMemo, useState } from "react";
import { Button, Collection, Tree, TreeItem, TreeItemContent, type Key } from "react-aria-components";
import { ChevronRight, MoreHorizontal } from "lucide-react";
import { StatusMark } from "@/lib/status";
import type { FileRow, SymbolRow } from "@/lib/join";
import { cn } from "@/lib/utils";

export type TreeRow = { kind: "file"; path: string; file: FileRow } | { kind: "symbol"; path: string; file: FileRow; symbol: SymbolRow };
type SortKey = "path" | "status" | "score";
const statusOrder = { served: 0, cut: 1, untouched: 2 } as const;

export function RepoTree({ rows, filter, onFocusRow, onAction }: {
  rows: FileRow[]; filter: string; onFocusRow: (row: TreeRow) => void; onAction: (row: TreeRow) => void;
}) {
  const [sort, setSort] = useState<SortKey>("path");
  const [expanded, setExpanded] = useState<Set<Key>>(() => new Set(rows.filter((r) => r.expandedByDefault).map((r) => r.path)));

  // Task 12 will re-render with new `rows` when a retrieval is selected; reopen the newly touched files.
  useEffect(() => {
    setExpanded(new Set(rows.filter((r) => r.expandedByDefault).map((r) => r.path)));
  }, [rows]);

  const visible = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const filtered = q
      ? rows.map((f) => ({ ...f, symbols: f.symbols.filter((s) => s.symbol.name.toLowerCase().includes(q)) }))
          .filter((f) => f.path.toLowerCase().includes(q) || f.symbols.length > 0)
      : rows;
    const sorted = [...filtered];
    if (sort === "path") sorted.sort((a, b) => a.path.localeCompare(b.path));
    if (sort === "status") sorted.sort((a, b) => (b.served + b.cut) - (a.served + a.cut) || a.path.localeCompare(b.path));
    if (sort === "score") sorted.sort((a, b) => Math.max(0, ...b.symbols.map((s) => s.item?.score ?? 0)) - Math.max(0, ...a.symbols.map((s) => s.item?.score ?? 0)));
    return sorted.map((f) => ({
      ...f,
      symbols: sort === "path" ? [...f.symbols].sort((a, b) => a.symbol.line_start - b.symbol.line_start)
        : sort === "status" ? [...f.symbols].sort((a, b) => statusOrder[a.status] - statusOrder[b.status] || a.symbol.line_start - b.symbol.line_start)
        : [...f.symbols].sort((a, b) => (b.item?.score ?? 0) - (a.item?.score ?? 0)),
    }));
  }, [rows, filter, sort]);

  const header = (k: SortKey, label: string) => (
    <button type="button" className={cn("px-2 py-1 text-left text-xs font-medium", sort === k && "underline")} aria-pressed={sort === k} onClick={() => setSort(k)}>
      Sort by {label}
    </button>
  );

  return (
    <div className="flex flex-col">
      <div className="flex gap-2 border-b" role="group" aria-label="Sort">{header("path", "path")}{header("status", "status")}{header("score", "score")}</div>
      <Tree
        aria-label="Repository"
        selectionMode="single"
        expandedKeys={expanded}
        onExpandedChange={(keys) => setExpanded(new Set(keys))}
        items={visible}
        className="outline-none"
      >
        {(file) => (
          <TreeItem id={file.path} textValue={file.path} className="outline-none data-[focused]:bg-accent">
            <TreeItemContent>
              <div className="flex min-h-6 items-center gap-2 px-2 py-1" onFocus={() => onFocusRow({ kind: "file", path: file.path, file })}>
                <Button slot="chevron" className="size-6 rounded data-[focused]:outline-2" aria-label={`Toggle ${file.path}`}>
                  <ChevronRight className="size-4 transition-none data-[expanded]:rotate-90" aria-hidden="true" />
                </Button>
                <span className="font-mono text-sm">{file.path}</span>
                <span className="text-xs text-muted-foreground">{file.lang ?? ""}</span>
                {file.served + file.cut > 0 && <span className="text-xs">{file.served} served · {file.cut} cut</span>}
                <Button className="ml-auto size-6 rounded" aria-label={`Actions for ${file.path}`} onPress={() => onAction({ kind: "file", path: file.path, file })}>
                  <MoreHorizontal className="size-4" aria-hidden="true" />
                </Button>
              </div>
            </TreeItemContent>
            <Collection items={file.symbols}>
              {(s) => (
                <TreeItem id={s.key} textValue={`${s.symbol.name} ${s.symbol.kind}`} className="outline-none data-[focused]:bg-accent">
                  <TreeItemContent>
                    <div className="flex min-h-6 items-center gap-2 py-1 pl-10 pr-2" onFocus={() => onFocusRow({ kind: "symbol", path: file.path, file, symbol: s })}>
                      <span className="font-mono text-sm">{s.symbol.name}</span>
                      <span className="text-xs text-muted-foreground">{s.symbol.kind} · line {s.symbol.line_start}</span>
                      <StatusMark status={s.status} />
                      {s.item && <span className="text-xs tabular-nums">{s.item.score.toFixed(2)}</span>}
                      <span className="truncate text-xs text-muted-foreground" title={s.symbol.signature}>{s.symbol.signature}</span>
                      <Button className="ml-auto size-6 rounded" aria-label={`Actions for ${s.symbol.name}`} onPress={() => onAction({ kind: "symbol", path: file.path, file, symbol: s })}>
                        <MoreHorizontal className="size-4" aria-hidden="true" />
                      </Button>
                    </div>
                  </TreeItemContent>
                </TreeItem>
              )}
            </Collection>
          </TreeItem>
        )}
      </Tree>
    </div>
  );
}
