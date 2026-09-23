import type { EntitiesPayload, Entity, Mention, Relation } from "@/api/types";

/** What an entity is called for assistive tech (map nodes, the panel's list): `<name> (<type>), N mentions`. */
export function entityAccessibleName(e: Pick<Entity, "name" | "type" | "mentions">): string {
  return `${e.name} (${e.type}), ${e.mentions} ${e.mentions === 1 ? "mention" : "mentions"}`;
}

/** The most-mentioned entities first; ties by name, so the list is stable. */
export function topEntities(p: EntitiesPayload, n = 10): Entity[] {
  return [...p.entities].sort((a, b) => b.mentions - a.mentions || a.name.localeCompare(b.name)).slice(0, n);
}

export const mentionsOf = (p: EntitiesPayload, id: number): Mention[] => p.mentions.filter((m) => m.entity_id === id);
export const relationsOf = (p: EntitiesPayload, id: number): Relation[] => p.relations.filter((r) => r.src === id || r.dst === id);

/** Entities a section mentions, in the payload's (mentions-descending) order. */
export function entitiesInSymbol(p: EntitiesPayload, symbolId: number): Entity[] {
  const ids = new Set(p.mentions.filter((m) => m.symbol_id === symbolId).map((m) => m.entity_id));
  return p.entities.filter((e) => ids.has(e.id));
}
export const relationsInSymbol = (p: EntitiesPayload, symbolId: number): Relation[] => p.relations.filter((r) => r.symbol_id === symbolId);

/** `src → dst: description`. A truncated payload may lack an endpoint: it reads as `entity <id>`. */
export function relationText(p: EntitiesPayload, r: Relation): string {
  const name = (id: number) => p.entities.find((e) => e.id === id)?.name ?? `entity ${id}`;
  return `${name(r.src)} → ${name(r.dst)}: ${r.description}`;
}

/** Symbol kinds that are prose, where extraction runs: only these list entities in the panel. */
export const PROSE_KINDS = new Set(["section", "document", "element"]);
