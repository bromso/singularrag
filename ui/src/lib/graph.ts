import Graph from "graphology";
import { circular } from "graphology-layout";
import forceAtlas2 from "graphology-layout-forceatlas2";
import noverlap from "graphology-layout-noverlap";
import type { EntitiesPayload, GraphPayload } from "@/api/types";

export type Positions = Record<string, { x: number; y: number }>;

export const entityKey = (id: number) => `entity:${id}`;

/** File nodes keyed by path; with `entities`, the knowledge overlay too: `entity:<id>` nodes,
 *  a `mention` edge to every file on the map that mentions the entity, and a `relation` edge
 *  between related entities. A mention of a file not on the map adds nothing. */
export function buildGraph(payload: GraphPayload, entities?: EntitiesPayload | null): Graph {
  const g = new Graph({ type: "undirected", multi: false, allowSelfLoops: false });
  for (const n of payload.nodes) g.addNode(n.path, { kind: "file", symbols: n.symbols, lang: n.lang, x: 0, y: 0, size: 1 });
  for (const e of payload.edges) {
    const a = payload.nodes[e.src]?.path, b = payload.nodes[e.dst]?.path;
    if (!a || !b || a === b) continue;
    const existing = g.edge(a, b);
    if (existing) {
      g.updateEdgeAttribute(existing, "weight", (w) => (w as number) + e.weight);
      // Summed, not deduped: a name referenced both ways (a->b and b->a) is counted once
      // per direction, so it is counted twice in the merged undirected edge's total.
      g.updateEdgeAttribute(existing, "names", (n) => (n as number) + e.names);
    } else {
      g.addEdge(a, b, { weight: e.weight, names: e.names });
    }
  }
  if (entities) addEntities(g, entities);
  return g;
}

function addEntities(g: Graph, { entities, relations, mentions }: EntitiesPayload) {
  for (const e of entities) {
    g.addNode(entityKey(e.id), { kind: "entity", entityId: e.id, label: e.name, entityType: e.type, mentions: e.mentions, description: e.description, x: 0, y: 0, size: 1 });
  }
  for (const m of mentions) {
    const k = entityKey(m.entity_id);
    // A file node's `kind` is "file": guards against a path that happens to look like a key.
    if (!g.hasNode(k) || !g.hasNode(m.path) || g.getNodeAttribute(m.path, "kind") !== "file") continue;
    if (!g.hasEdge(k, m.path)) g.addEdge(k, m.path, { kind: "mention", weight: 0.5, names: 0 });
  }
  for (const r of relations) {
    const a = entityKey(r.src), b = entityKey(r.dst);
    if (a === b || !g.hasNode(a) || !g.hasNode(b) || g.hasEdge(a, b)) continue;
    g.addEdge(a, b, { kind: "relation", weight: 1, names: 0, description: r.description });
  }
}

const ITERATIONS = 300;

/** `graphology-layout-noverlap` has no notion of a pinned/`fixed` node, so running it after a
 *  seeded ForceAtlas2 pass could nudge survivors off their cached position. It also jitters
 *  exactly-coincident nodes with `Math.random()`, which breaks determinism. So: separate any
 *  coincident coordinates ourselves (deterministically) before it runs, and skip it entirely
 *  when a seed is present — survivors must keep their cached positions exactly, and newcomers
 *  were already placed at neighbour centroids, so there is nothing left for noverlap to fix. */
export function separateCoincident(graph: Graph): void {
  const seen = new Map<string, number>();
  graph.forEachNode((node, attrs) => {
    const key = `${Math.round(attrs.x * 1e6)}:${Math.round(attrs.y * 1e6)}`;
    const countBefore = seen.get(key) ?? 0;
    seen.set(key, countBefore + 1);
    if (countBefore > 0) {
      const k = countBefore + 1; // 1-based index within the coincident group, in insertion order
      graph.mergeNodeAttributes(node, { x: attrs.x + k * 0.01, y: attrs.y + k * 0.01 });
    }
  });
}

/** Circular seed (or the previous positions for files that still exist), ForceAtlas2, then
 *  a no-overlap pass. Deterministic: no randomness anywhere. */
export function layoutGraph(graph: Graph, seed?: Positions): Positions {
  const g = graph.copy();
  circular.assign(g, { scale: 100 });
  if (seed) {
    let cx = 0, cy = 0, n = 0;
    g.forEachNode((node) => {
      const s = seed[node];
      if (s) { g.mergeNodeAttributes(node, { x: s.x, y: s.y, fixed: true }); cx += s.x; cy += s.y; n += 1; }
    });
    if (n > 0) {
      // Newcomers start at the centroid of their seeded neighbours, else at the seed centroid.
      g.forEachNode((node) => {
        if (seed[node]) return;
        let nx = 0, ny = 0, k = 0;
        g.forEachNeighbor(node, (nb) => { const s = seed[nb]; if (s) { nx += s.x; ny += s.y; k += 1; } });
        g.mergeNodeAttributes(node, k > 0 ? { x: nx / k, y: ny / k } : { x: cx / n, y: cy / n });
      });
    }
  }
  if (g.order > 1) {
    // Seeded nodes are pinned (`fixed`) during ForceAtlas2 so existing files keep their
    // on-screen position across a refresh; only new/unseeded nodes get pulled into place.
    forceAtlas2.assign(g, { iterations: ITERATIONS, settings: { ...forceAtlas2.inferSettings(g), gravity: 1, scalingRatio: 10 } });
    if (seed) {
      // Skip no-overlap entirely: survivors must keep their exact cached positions, and
      // noverlap doesn't know how to leave pinned nodes alone.
    } else {
      separateCoincident(g);
      noverlap.assign(g, { maxIterations: 50, settings: { margin: 4 } });
    }
  }
  if (seed) g.forEachNode((node) => { if (seed[node]) g.setNodeAttribute(node, "fixed", false); });
  const out: Positions = {};
  g.forEachNode((node, attrs) => { out[node] = { x: attrs.x, y: attrs.y }; });
  return out;
}

export function applyPositions(graph: Graph, positions: Positions): void {
  graph.forEachNode((node) => {
    const p = positions[node] ?? { x: 0, y: 0 };
    graph.mergeNodeAttributes(node, { x: p.x, y: p.y });
  });
}

const LAYOUT_CACHE_PREFIX = "singularrag.layout.";
export const layoutCacheKey = (indexVersion: string) => `${LAYOUT_CACHE_PREFIX}${indexVersion}`;

function isValidPositions(v: unknown): v is Positions {
  if (!v || typeof v !== "object" || Array.isArray(v)) return false;
  return Object.values(v as Record<string, unknown>).every(
    (p) => p !== null && typeof p === "object" && !Array.isArray(p)
      && Number.isFinite((p as { x: unknown }).x) && Number.isFinite((p as { y: unknown }).y),
  );
}

export function loadLayout(indexVersion: string): Positions | null {
  try {
    const raw = localStorage.getItem(layoutCacheKey(indexVersion));
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    return isValidPositions(parsed) ? parsed : null;
  } catch { return null; }
}

/** Only the newest layout is ever needed (the cache is keyed by index version, and a new
 *  version invalidates the old one), so every other `singularrag.layout.*` key is evicted
 *  before writing — otherwise this grows without bound across re-indexes. */
export function saveLayout(indexVersion: string, positions: Positions): void {
  try {
    const key = layoutCacheKey(indexVersion);
    for (let i = localStorage.length - 1; i >= 0; i -= 1) {
      const k = localStorage.key(i);
      if (k && k.startsWith(LAYOUT_CACHE_PREFIX) && k !== key) localStorage.removeItem(k);
    }
    localStorage.setItem(key, JSON.stringify(positions));
  } catch {}
}
