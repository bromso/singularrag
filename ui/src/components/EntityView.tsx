import { Button } from "@/components/ui/button";
import type { EntitiesPayload, Entity } from "@/api/types";
import { entitiesInSymbol, entityAccessibleName, mentionsOf, relationText, relationsInSymbol, relationsOf, topEntities } from "@/lib/entities";

/** Focus the section a mention points at: by symbol id first, then by path and name. */
export type FocusSection = (path: string, name: string, symbolId?: number) => void;

/** The detail panel's content for one entity (the panel supplies the section, so the
 *  region element is the same one across row, entity and empty states): type, description, where it is mentioned (each
 *  with a way to go there), and its relations. Everything is text; no colour-only signal. */
export function EntityView({ entity, entities, onFocusSection }: { entity: Entity; entities: EntitiesPayload | null; onFocusSection: FocusSection }) {
  const mentions = entities ? mentionsOf(entities, entity.id) : [];
  const relations = entities ? relationsOf(entities, entity.id) : [];
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
        {mentions.length === 0 ? <p className="text-sm text-muted-foreground">No mentions on record.</p> : (
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
        {relations.length === 0 ? <p className="text-sm text-muted-foreground">No relations on record.</p> : (
          <ul aria-labelledby="entity-relations-heading" className="list-disc pl-5 text-sm">
            {relations.map((r) => <li key={r.id}>{relationText(entities!, r)}</li>)}
          </ul>
        )}
      </div>
    </>
  );
}

/** The panel's empty state lists the most-mentioned entities: the keyboard path to an
 *  entity (a click on the map is the mouse path). */
export function EntityList({ entities, onFocusEntity }: { entities: EntitiesPayload | null; onFocusEntity: (id: number) => void }) {
  const top = entities ? topEntities(entities, 10) : [];
  if (top.length === 0) return null;
  return (
    <div className="mt-3">
      <h2 id="entity-list-heading" className="text-sm font-semibold text-foreground">Entities</h2>
      <ul aria-labelledby="entity-list-heading" className="mt-1 flex flex-col items-start gap-1">
        {top.map((e) => (
          <li key={e.id}>
            <Button type="button" variant="link" size="sm" className="h-auto px-0 text-foreground" onClick={() => onFocusEntity(e.id)}>{entityAccessibleName(e)}</Button>
          </li>
        ))}
      </ul>
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
