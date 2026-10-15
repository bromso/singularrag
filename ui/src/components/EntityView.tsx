import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { api } from "@/api/client";
import type { EntitiesPayload, Entity, EntityDetail, Relation } from "@/api/types";
import { entitiesInSymbol, entityAccessibleName, mentionsOf, relationText, relationsInSymbol, relationsOf, topEntities } from "@/lib/entities";
import { useDebounced } from "@/lib/useDebounced";

/** Focus the section a mention points at: by symbol id first, then by path and name. */
export type FocusSection = (path: string, name: string, symbolId?: number) => void;

/** Focus an entity; `entity` is passed when it may lie outside the overlay (a search result). */
export type FocusEntity = (id: number, entity?: Entity) => void;

type DetailState = { id: number; detail: EntityDetail | null; failed: boolean };

/** The detail panel's content for one entity (the panel supplies the section, so the
 *  region element is the same one across row, entity and empty states): type, description, where it is mentioned (each
 *  with a way to go there), its relations and, for a process, its steps. The lists come from
 *  the entity's own detail query, uncapped; until it answers they read "Loading…", and if it
 *  fails they fall back to the overlay. Everything is text; no colour-only signal. */
export function EntityView({ entity, entities, onFocusSection }: { entity: Entity; entities: EntitiesPayload | null; onFocusSection: FocusSection }) {
  const [state, setState] = useState<DetailState | null>(null);
  useEffect(() => {
    let live = true;
    setState({ id: entity.id, detail: null, failed: false });
    api.entityDetail(entity.id).then(
      (detail) => { if (live) setState({ id: entity.id, detail, failed: false }); },
      () => { if (live) setState({ id: entity.id, detail: null, failed: true }); },
    );
    return () => { live = false; };
  }, [entity.id]);
  const current = state?.id === entity.id ? state : null;
  const detail = current?.detail ?? null;
  const loading = !current || (!current.detail && !current.failed);
  const mentions = detail ? detail.mentions : entities ? mentionsOf(entities, entity.id) : [];
  const relations = detail ? detail.relations : entities ? relationsOf(entities, entity.id) : [];
  // Relation ends are named from the overlay plus this entity itself; an end outside both reads `entity <id>`.
  const names: EntitiesPayload = { entities: [entity, ...(entities?.entities ?? [])], relations: [], mentions: [], truncated: false };
  const steps = detail?.steps ?? null;
  return (
    <>
      <h2 id="detail-heading" tabIndex={-1} className="text-sm font-semibold">{entity.name}</h2>
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 text-sm">
        <dt className="text-muted-foreground">Type</dt><dd>{entity.type}</dd>
        <dt className="text-muted-foreground">Mentions</dt><dd>{entity.mentions}</dd>
      </dl>
      <p className="text-sm">{entity.description || "No description."}</p>
      <div>
        <h3 id="entity-mentions-heading" className="text-sm font-semibold">Mentioned in</h3>
        {loading ? <p className="text-sm text-muted-foreground">Loading…</p> : mentions.length === 0 ? <p className="text-sm text-muted-foreground">No mentions on record.</p> : (
          <ul aria-labelledby="entity-mentions-heading" className="flex flex-col gap-1 text-sm">
            {mentions.map((m) => (
              <li key={`${m.symbol_id}`} className="flex flex-wrap items-center gap-2">
                <span className="font-mono">{m.path} :: {m.name}</span>
                <Button type="button" variant="outline" size="sm" aria-label={`Go to section ${m.name} in ${m.path}`} onClick={() => onFocusSection(m.path, m.name, m.symbol_id)}>
                  Go to section
                </Button>
              </li>
            ))}
          </ul>
        )}
      </div>
      <div>
        <h3 id="entity-relations-heading" className="text-sm font-semibold">Relations</h3>
        {loading ? <p className="text-sm text-muted-foreground">Loading…</p> : relations.length === 0 ? <p className="text-sm text-muted-foreground">No relations on record.</p> : (
          <ul aria-labelledby="entity-relations-heading" className="list-disc pl-5 text-sm">
            {relations.map((r: Relation) => <li key={r.id}>{relationText(names, r)}</li>)}
          </ul>
        )}
      </div>
      {steps && steps.length > 0 && (
        <div>
          <h3 className="text-sm font-semibold">Steps</h3>
          <ol aria-label={`Steps of ${entity.name}`} className="list-decimal pl-5 text-sm">
            {steps.map((s) => <li key={s.ordinal}>{s.role ? `${s.text} (${s.role})` : s.text}</li>)}
          </ol>
        </div>
      )}
    </>
  );
}

/** The panel's empty state lists the most-mentioned entities: the keyboard path to an
 *  entity (a click on the map is the mouse path). `Find an entity` searches every entity by
 *  name (debounced), including those past the overlay's cap; an empty field shows the top 10. */
export function EntityList({ entities, onFocusEntity }: { entities: EntitiesPayload | null; onFocusEntity: FocusEntity }) {
  const top = entities ? topEntities(entities, 10) : [];
  const [text, setText] = useState("");
  const q = useDebounced(text.trim(), 200);
  const [found, setFound] = useState<{ q: string; entities: Entity[] } | null>(null);
  const gen = useRef(0);
  useEffect(() => {
    const my = ++gen.current;
    if (!q) { setFound(null); return; }
    api.entitySearch(q).then(
      (r) => { if (my === gen.current) setFound({ q, entities: r.entities }); },
      () => { if (my === gen.current) setFound({ q, entities: [] }); },
    );
  }, [q]);
  if (top.length === 0 && !text) return null;
  const searching = q !== "" && found?.q !== q;
  const shown = q ? (found?.q === q ? found.entities : []) : top;
  return (
    <div className="mt-3">
      <h2 id="entity-list-heading" className="text-sm font-semibold text-foreground">Entities</h2>
      <label className="mt-1 block text-sm">Find an entity
        <input type="search" className="mt-1 block w-full rounded border bg-background px-2 py-1" value={text} onChange={(e) => setText(e.target.value)} />
      </label>
      {searching ? <p className="mt-1 text-sm text-muted-foreground">Loading…</p> : shown.length === 0 ? <p className="mt-1 text-sm text-muted-foreground">No entities match.</p> : (
        <ul aria-labelledby="entity-list-heading" className="mt-1 flex flex-col items-start gap-1">
          {shown.map((e) => (
            <li key={e.id}>
              {/* The description the map's hover box shows, as visible text here too, so the
                  keyboard path carries no less than the mouse path. */}
              <Button type="button" variant="link" size="sm" className="h-auto px-0 text-foreground" aria-describedby={e.description ? `entity-desc-${e.id}` : undefined} onClick={() => onFocusEntity(e.id, e)}>{entityAccessibleName(e)}</Button>
              {e.description && <p id={`entity-desc-${e.id}`} className="text-xs text-muted-foreground">{e.description}</p>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** For a prose symbol (section, document, element): the entities it mentions and the
 *  relations it states. Nothing when there are none. */
export function SymbolKnowledge({ symbolId, entities, onFocusEntity }: { symbolId: number; entities: EntitiesPayload | null; onFocusEntity: (id: number) => void }) {
  if (!entities) return null;
  const ents = entitiesInSymbol(entities, symbolId);
  const rels = relationsInSymbol(entities, symbolId);
  if (ents.length === 0 && rels.length === 0) return null;
  return (
    <>
      {ents.length > 0 && (
        <div>
          <h3 id="symbol-entities-heading" className="text-sm font-semibold">Entities in this section</h3>
          <ul aria-labelledby="symbol-entities-heading" className="flex flex-wrap gap-1">
            {ents.map((e) => (
              <li key={e.id}>
                <Button type="button" variant="outline" size="sm" onClick={() => onFocusEntity(e.id)}>{`${e.name} (${e.type})`}</Button>
              </li>
            ))}
          </ul>
        </div>
      )}
      {rels.length > 0 && (
        <div>
          <h3 id="symbol-relations-heading" className="text-sm font-semibold">Relations stated here</h3>
          <ul aria-labelledby="symbol-relations-heading" className="list-disc pl-5 text-sm">
            {rels.map((r) => <li key={r.id}>{relationText(entities, r)}</li>)}
          </ul>
        </div>
      )}
    </>
  );
}
