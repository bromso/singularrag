import { useEffect, useMemo, useRef, useState } from "react";
import { Button, Collection, Tree, TreeItem, TreeItemContent, type Key } from "react-aria-components";
import { ChevronRight, MoreHorizontal } from "lucide-react";
import { StatusMark } from "@/lib/status";
import type { FileRow, SymbolRow } from "@/lib/join";
import { reasonsToSentences } from "@/lib/reasons";
import { cn } from "@/lib/utils";

export type TreeRow = { kind: "file"; path: string; file: FileRow } | { kind: "symbol"; path: string; file: FileRow; symbol: SymbolRow };
// The accent tint alone is ~1.07:1 against the background — not a focus indicator (I4).
// `outline-none` is deliberately absent here (it stays on the Tree container); react-aria
// sets data-focus-visible on the row only for keyboard focus.
const FOCUSABLE_ROW =
  "data-[focused]:bg-accent data-[focus-visible]:outline-2 data-[focus-visible]:outline-offset-[-2px] data-[focus-visible]:outline-ring";
type SortKey = "path" | "status" | "score";
const statusOrder = { served: 0, cut: 1, untouched: 2 } as const;

// react-aria-components' <TreeItem> does not forward `onFocus` (focus events are
// intentionally excluded from its DOM prop passthrough — react-aria manages focus
// itself). The supported way to observe a row's focus state is the `isFocused`
// render prop exposed by <TreeItemContent>'s children-as-function; these two
// components consume that and report focus via a real effect.
function FileRowView({ file, isFocused, onFocusRow, onAction }: {
  file: FileRow; isFocused: boolean; onFocusRow: (row: TreeRow) => void; onAction: (row: TreeRow) => void;
}) {
  useEffect(() => {
    if (isFocused) onFocusRow({ kind: "file", path: file.path, file });
  }, [isFocused, file, onFocusRow]);
  return (
    <div className="flex min-h-6 items-center gap-2 px-2 py-1">
      <Button slot="chevron" className="size-6 rounded data-[focused]:outline-2" aria-label={`Toggle ${file.path}`}>
        <ChevronRight className="size-4 transition-none data-[expanded]:rotate-90" aria-hidden="true" />
      </Button>
      <span className="font-mono text-sm">{file.path}</span>
      <span className="text-xs text-muted-foreground">{file.lang ?? ""}</span>
      {file.served + file.cut > 0 && <span className="text-xs">{file.served} served · {file.cut} cut</span>}
      <Button className="ml-auto size-6 rounded" aria-label={`Actions for ${file.path}`} aria-controls="detail-panel" onPress={() => onAction({ kind: "file", path: file.path, file })}>
        <MoreHorizontal className="size-4" aria-hidden="true" />
      </Button>
    </div>
  );
}

function SymbolRowView({ file, symbol, isFocused, onFocusRow, onAction }: {
  file: FileRow; symbol: SymbolRow; isFocused: boolean; onFocusRow: (row: TreeRow) => void; onAction: (row: TreeRow) => void;
}) {
  useEffect(() => {
    if (isFocused) onFocusRow({ kind: "symbol", path: file.path, file, symbol });
  }, [isFocused, file, symbol, onFocusRow]);
  const reason = symbol.item ? (reasonsToSentences(symbol.item.reasons, symbol.item.rank, symbol.item.score)[1] ?? "") : "";
  return (
    <div className="flex min-h-6 items-center gap-2 py-1 pl-10 pr-2">
      <span className="font-mono text-sm">{symbol.symbol.name}</span>
      <span className="text-xs text-muted-foreground">{symbol.symbol.kind} · line {symbol.symbol.line_start}</span>
      <StatusMark status={symbol.status} />
      {symbol.moved && <span className="text-xs text-muted-foreground">moved</span>}
      {symbol.item && <span className="text-xs tabular-nums">{symbol.item.score.toFixed(2)}</span>}
      {symbol.item && <span className="truncate text-xs text-muted-foreground">{reason}</span>}
      <span className="truncate text-xs text-muted-foreground" title={symbol.symbol.signature}>{symbol.symbol.signature}</span>
      <Button className="ml-auto size-6 rounded" aria-label={`Actions for ${symbol.symbol.name}`} aria-controls="detail-panel" onPress={() => onAction({ kind: "symbol", path: file.path, file, symbol })}>
        <MoreHorizontal className="size-4" aria-hidden="true" />
      </Button>
    </div>
  );
}

const defaultExpansion = (rows: FileRow[]) => new Set<Key>(rows.filter((r) => r.expandedByDefault).map((r) => r.path));

export function RepoTree({ rows, filter, seedKey, onFocusRow, onAction }: {
  rows: FileRow[]; filter: string; seedKey: number; onFocusRow: (row: TreeRow) => void; onAction: (row: TreeRow) => void;
}) {
  const [sort, setSort] = useState<SortKey>("path");
  const [expanded, setExpanded] = useState<Set<Key>>(() => defaultExpansion(rows));

  // The expansion is seeded by the *retrieval*, not by the rows: a live "change" event
  // hands down a fresh `rows` array several times a minute, and re-seeding on that would
  // collapse whatever the user had opened and move focus mid-navigation (I2). `seedKey`
  // is the selected retrieval's id, so this runs only when the selection lands.
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  useEffect(() => {
    setExpanded(defaultExpansion(rowsRef.current));
  }, [seedKey]);

  // New rows may have dropped files (a delete, a rename); forget their keys, keep the rest.
  useEffect(() => {
    setExpanded((prev) => {
      const alive = new Set<Key>(rows.map((r) => r.path));
      const next = new Set([...prev].filter((k) => alive.has(k)));
      return next.size === prev.size ? prev : next;
    });
  }, [rows]);

  const visible = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const filtered = q
      ? rows.map((f) => ({ ...f, symbols: f.symbols.filter((s) => s.symbol.name.toLowerCase().includes(q)) }))
          .filter((f) => f.path.toLowerCase().includes(q) || f.symbols.length > 0)
      : rows;
    const sorted = [...filtered];
    if (sort === "path") sorted.sort((a, b) => Number(b.served + b.cut > 0) - Number(a.served + a.cut > 0) || a.path.localeCompare(b.path));
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
          <TreeItem id={file.path} textValue={file.path} className={FOCUSABLE_ROW}>
            <TreeItemContent>
              {({ isFocused }) => <FileRowView file={file} isFocused={isFocused} onFocusRow={onFocusRow} onAction={onAction} />}
            </TreeItemContent>
            <Collection items={file.symbols}>
              {(s) => (
                <TreeItem id={s.key} textValue={`${s.symbol.name} ${s.symbol.kind}`} className={FOCUSABLE_ROW}>
                  <TreeItemContent>
                    {({ isFocused }) => <SymbolRowView file={file} symbol={s} isFocused={isFocused} onFocusRow={onFocusRow} onAction={onAction} />}
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
