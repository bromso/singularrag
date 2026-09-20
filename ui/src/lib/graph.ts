import Graph from "graphology";
import { circular } from "graphology-layout";
import forceAtlas2 from "graphology-layout-forceatlas2";
import noverlap from "graphology-layout-noverlap";
import type { GraphPayload } from "@/api/types";

export type Positions = Record<string, { x: number; y: number }>;

export function buildGraph(payload: GraphPayload): Graph {
  const g = new Graph({ type: "undirected", multi: false, allowSelfLoops: false });
  for (const n of payload.nodes) g.addNode(n.path, { symbols: n.symbols, lang: n.lang, x: 0, y: 0, size: 1 });
  for (const e of payload.edges) {
    const a = payload.nodes[e.src]?.path, b = payload.nodes[e.dst]?.path;
    if (!a || !b || a === b) continue;
    const existing = g.edge(a, b);
    if (existing) {
      g.updateEdgeAttribute(existing, "weight", (w) => (w as number) + e.weight);
      g.updateEdgeAttribute(existing, "names", (n) => Math.max(n as number, e.names));
    } else {
      g.addEdge(a, b, { weight: e.weight, names: e.names });
    }
  }
  return g;
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

export const layoutCacheKey = (indexVersion: string) => `singularrag.layout.${indexVersion}`;

export function loadLayout(indexVersion: string): Positions | null {
  try {
    const raw = localStorage.getItem(layoutCacheKey(indexVersion));
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? (parsed as Positions) : null;
  } catch { return null; }
}

export function saveLayout(indexVersion: string, positions: Positions): void {
  try { localStorage.setItem(layoutCacheKey(indexVersion), JSON.stringify(positions)); } catch {}
}
