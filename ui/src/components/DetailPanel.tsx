import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import type { MapConfig } from "@/api/types";
import { reasonsToSentences } from "@/lib/reasons";
import { isExcluded, isPinned, noteFor } from "@/lib/mapEdits";
import type { TreeRow } from "./RepoTree";

export function DetailPanel({ row, map, onPin, onExclude, onNote }: {
  row: TreeRow | null; map: MapConfig;
  onPin: (path: string, symbol?: string) => void; onExclude: (path: string) => void; onNote: (path: string, symbol: string | undefined, text: string) => void;
}) {
  const symbol = row?.kind === "symbol" ? row.symbol.symbol.name : undefined;
  const [text, setText] = useState("");
  const saved = row ? noteFor(map, row.path, symbol) : "";
  // A draft the user hasn't blurred yet must survive a background refetch of `map`
  // (e.g. from an SSE "change" event triggered by another agent). `dirty` tracks
  // whether the textarea has unsaved edits; the effect below only overwrites
  // `text` from `saved` when the target (row/symbol) actually changed, or when
  // it didn't but the field is clean.
  const dirtyRef = useRef(false);
  const lastKeyRef = useRef<string | null>(null);
  useEffect(() => {
    const key = row ? `${row.path}::${symbol ?? ""}` : null;
    const targetChanged = key !== lastKeyRef.current;
    lastKeyRef.current = key;
    if (targetChanged) {
      dirtyRef.current = false;
      setText(saved);
    } else if (!dirtyRef.current) {
      setText(saved);
    }
  }, [row?.path, symbol, saved]);
  if (!row) return <section aria-label="Details" className="border-l p-3 text-sm text-muted-foreground">Select a file or symbol.</section>;
  const item = row.kind === "symbol" ? row.symbol.item : null;
  return (
    <section aria-label="Details" className="flex flex-col gap-3 border-l p-3">
      <h2 className="font-mono text-sm font-semibold">{symbol ? `${row.path} :: ${symbol}` : row.path}</h2>
      {item ? (
        <ul className="list-disc pl-5 text-sm">{reasonsToSentences(item.reasons, item.rank, item.score).map((s) => <li key={s}>{s}</li>)}</ul>
      ) : (
        <p className="text-sm text-muted-foreground">{row.kind === "symbol" ? "Not part of the selected retrieval." : `${row.file.served} served · ${row.file.cut} cut`}</p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="outline" aria-pressed={isPinned(map, row.path, symbol)} onClick={() => onPin(row.path, symbol)}>
          {isPinned(map, row.path, symbol) ? "Unpin" : symbol ? "Pin symbol" : "Pin file"}
        </Button>
        <Button type="button" variant="outline" aria-pressed={isExcluded(map, row.path)} onClick={() => onExclude(row.path)}>
          {isExcluded(map, row.path) ? "Include file" : "Exclude file"}
        </Button>
      </div>
      <label className="text-sm">
        Note
        <Textarea
          value={text}
          onChange={(e) => { setText(e.target.value); dirtyRef.current = true; }}
          onBlur={() => { onNote(row.path, symbol, text); dirtyRef.current = false; }}
          rows={3}
          className="mt-1"
        />
      </label>
      {(isPinned(map, row.path, symbol) || isExcluded(map, row.path)) && (
        <p className="text-xs text-muted-foreground">{isPinned(map, row.path, symbol) ? "Pinned. " : ""}{isExcluded(map, row.path) ? "Excluded." : ""}</p>
      )}
    </section>
  );
}
