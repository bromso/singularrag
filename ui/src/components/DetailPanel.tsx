import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import type { BlastResult, EntitiesPayload, MapConfig } from "@/api/types";
import { PROSE_KINDS } from "@/lib/entities";
import { reasonsToSentences } from "@/lib/reasons";
import { agentNoteFor, boundariesOf, boundaryNames, isExcluded, isPinned, noteFor } from "@/lib/mapEdits";
import { EntityList, EntityView, SymbolKnowledge, type FocusSection } from "./EntityView";
import type { TreeRow } from "./RepoTree";

export function DetailPanel({ row: focused, map, entities, onFocusEntity, onFocusSection, onPin, onExclude, onNote, onRemoveAgentNote, blast, blastLoading, onToggleBlast, expandedPath, onToggleExpand, onAddBoundary, onRemoveBoundary }: {
  row: TreeRow | null; map: MapConfig;
  onPin: (path: string, symbol?: string) => void; onExclude: (path: string) => void; onNote: (path: string, symbol: string | undefined, text: string) => void;
  onRemoveAgentNote: (path: string, symbol?: string) => void;
  blast: BlastResult | null; blastLoading: boolean; onToggleBlast: (path: string, symbol: string) => void;
  expandedPath: string | null; onToggleExpand: (path: string) => void;
  onAddBoundary: (name: string, path: string) => void; onRemoveBoundary: (name: string, path: string) => void;
  entities: EntitiesPayload | null; onFocusEntity: (id: number) => void; onFocusSection: FocusSection;
}) {
  // A focused entity has its own view; the hooks below (the note draft) are about a file or symbol.
  const row = focused?.kind === "entity" ? null : focused;
  const [newBoundary, setNewBoundary] = useState("");
  const symbol = row?.kind === "symbol" ? row.symbol.symbol.name : undefined;
  const [text, setText] = useState("");
  const saved = row ? noteFor(map, row.path, symbol) : "";
  const agentNote = row ? agentNoteFor(map, row.path, symbol) : undefined;
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
  if (focused?.kind === "entity") {
    return (
      <section id="detail-panel" aria-label="Details" className="flex min-h-0 flex-col gap-3 overflow-auto border-l p-3">
        <EntityView entity={focused.entity} entities={entities} onFocusSection={onFocusSection} />
      </section>
    );
  }
  if (!row) {
    return (
      <section id="detail-panel" aria-label="Details" className="min-h-0 overflow-auto border-l p-3 text-sm">
        <p className="text-muted-foreground">Select a file or symbol.</p>
        <EntityList entities={entities} onFocusEntity={onFocusEntity} />
      </section>
    );
  }
  const item = row.kind === "symbol" ? row.symbol.item : null;
  const blastShown = row.kind === "symbol" && !!blast && blast.root.path === row.path && blast.root.symbol === row.symbol.symbol.name;
  return (
    <section id="detail-panel" aria-label="Details" className="flex min-h-0 flex-col gap-3 overflow-auto border-l p-3">
      {/* The row action buttons are `aria-controls="detail-panel"` and move focus here,
          so a screen-reader user lands on what they just opened (I10). */}
      <h2 id="detail-heading" tabIndex={-1} className="font-mono text-sm font-semibold">{symbol ? `${row.path} :: ${symbol}` : row.path}</h2>
      {item ? (
        <>
          <ul className="list-disc pl-5 text-sm">{reasonsToSentences(item.reasons, item.rank, item.score).map((s) => <li key={s}>{s}</li>)}</ul>
          {row.kind === "symbol" && row.symbol.moved && <p className="text-sm text-muted-foreground">Moved since this retrieval (was line {item.line_start})</p>}
        </>
      ) : (
        <p className="text-sm text-muted-foreground">{row.kind === "symbol" ? "Not part of the selected retrieval." : `${row.file.served} served · ${row.file.cut} cut`}</p>
      )}
      {row.kind === "symbol" && PROSE_KINDS.has(row.symbol.symbol.kind) && (
        <SymbolKnowledge symbolId={row.symbol.symbol.id} entities={entities} onFocusEntity={onFocusEntity} />
      )}
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="outline" aria-pressed={isPinned(map, row.path, symbol)} onClick={() => onPin(row.path, symbol)}>
          {isPinned(map, row.path, symbol) ? "Unpin" : symbol ? "Pin symbol" : "Pin file"}
        </Button>
        <Button type="button" variant="outline" aria-pressed={isExcluded(map, row.path)} onClick={() => onExclude(row.path)}>
          {isExcluded(map, row.path) ? "Include file" : "Exclude file"}
        </Button>
      </div>
      <div role="group" aria-labelledby="boundaries-label" className="flex flex-wrap items-center gap-2 text-sm">
        <span id="boundaries-label">Boundaries</span>
        {boundariesOf(map, row.path).map((name) => (
          <span key={name} className="inline-flex items-center gap-1 rounded border px-2 py-0.5">
            {name}
            <Button type="button" variant="ghost" size="sm" aria-label={`Remove from ${name}`} onClick={() => onRemoveBoundary(name, row.path)}>×</Button>
          </span>
        ))}
        <form className="inline-flex items-center gap-1" onSubmit={(e) => {
          e.preventDefault();
          const n = newBoundary.trim();
          setNewBoundary("");
          if (!n || boundariesOf(map, row.path).includes(n)) return;
          onAddBoundary(newBoundary, row.path);
        }}>
          <label className="sr-only" htmlFor="boundary-input">Add to boundary</label>
          <input id="boundary-input" list="boundary-names" value={newBoundary} onChange={(e) => setNewBoundary(e.target.value)} className="w-32 rounded border bg-background px-2 py-1" placeholder="boundary name" />
          <datalist id="boundary-names">{boundaryNames(map).map((n) => <option key={n} value={n} />)}</datalist>
          <Button type="submit" variant="outline" size="sm">Add</Button>
        </form>
      </div>
      {row.kind === "file" && (
        <Button type="button" variant="outline" aria-pressed={expandedPath === row.path} onClick={() => onToggleExpand(row.path)}>
          {expandedPath === row.path ? "Hide symbols" : "Show symbols"}
        </Button>
      )}
      {row.kind === "symbol" && (
        <Button type="button" variant="outline" aria-pressed={blastShown} onClick={() => onToggleBlast(row.path, row.symbol.symbol.name)} aria-busy={blastLoading}>
          {blastShown ? "Hide blast radius" : "Show blast radius"}
        </Button>
      )}
      {blastShown && blast && (
        <div>
          <h3 id="blast-heading" className="text-sm font-semibold">Blast radius</h3>
          {blast.files.length === 0 ? <p className="text-sm text-muted-foreground">Nothing references this symbol.</p> : (
            <ol aria-labelledby="blast-heading" className="list-decimal pl-5 text-sm">
              {blast.files.map((f) => <li key={f.path}>{f.path} (depth {f.depth}, via {f.via})</li>)}
            </ol>
          )}
          {blast.truncated && <p className="text-xs text-muted-foreground">{blast.truncated === "depth" ? "Stopped at the depth cap" : "Stopped at 200 files"}</p>}
        </div>
      )}
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
      {agentNote && (
        <div className="rounded border p-2 text-sm">
          <p className="flex items-center gap-2">
            <span className="rounded bg-muted px-1 text-xs font-medium">agent</span>
            <time dateTime={agentNote.at} className="text-xs text-muted-foreground">{agentNote.at}</time>
          </p>
          <p className="mt-1">{agentNote.text}</p>
          <Button type="button" variant="outline" size="sm" className="mt-2" aria-label="Delete the agent note" onClick={() => onRemoveAgentNote(row.path, symbol)}>
            Delete
          </Button>
        </div>
      )}
      {(isPinned(map, row.path, symbol) || isExcluded(map, row.path)) && (
        <p className="text-xs text-muted-foreground">{isPinned(map, row.path, symbol) ? "Pinned. " : ""}{isExcluded(map, row.path) ? "Excluded." : ""}</p>
      )}
    </section>
  );
}
