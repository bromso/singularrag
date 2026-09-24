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

const HOVER_WIDTH = 48;
const HOVER_DESCRIPTION_LINES = 4;

/** Word-wrap to `width` characters; a word longer than a line is split. */
function wrap(text: string, width: number): string[] {
  const lines: string[] = [];
  let line = "";
  for (let word of text.split(/\s+/).filter(Boolean)) {
    while (word.length > width) {
      if (line) { lines.push(line); line = ""; }
      lines.push(word.slice(0, width));
      word = word.slice(width);
    }
    if (!word) continue;
    if (!line) line = word;
    else if (line.length + 1 + word.length <= width) line += ` ${word}`;
    else { lines.push(line); line = word; }
  }
  if (line) lines.push(line);
  return lines;
}

/** What the map's hover box shows for a node: its label, and for an entity its description
 *  under it, wrapped to a legible width and cut to four lines with an ellipsis. */
export function hoverLines(data: { label?: string | null; kind?: unknown; description?: unknown }): string[] {
  if (typeof data.label !== "string") return [];
  if (data.kind !== "entity" || typeof data.description !== "string" || !data.description.trim()) return [data.label];
  const desc = wrap(data.description, HOVER_WIDTH);
  if (desc.length > HOVER_DESCRIPTION_LINES) {
    desc.length = HOVER_DESCRIPTION_LINES;
    desc[HOVER_DESCRIPTION_LINES - 1] = `${desc[HOVER_DESCRIPTION_LINES - 1].slice(0, HOVER_WIDTH - 1)}…`;
  }
  return [data.label, ...desc];
}
