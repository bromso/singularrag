import { describe, expect, test, beforeEach } from "bun:test";
import { applyPositions, buildGraph, layoutCacheKey, layoutGraph, loadLayout, saveLayout, separateCoincident } from "./graph";
import type { GraphPayload } from "@/api/types";

const payload: GraphPayload = {
  index_version: "v1",
  nodes: [{ path: "a.ts", symbols: 3, lang: "typescript" }, { path: "b.ts", symbols: 1, lang: null }, { path: "c.ts", symbols: 9, lang: "typescript" }],
  edges: [{ src: 1, dst: 0, weight: 1, names: 1 }, { src: 0, dst: 1, weight: 0.5, names: 2 }, { src: 2, dst: 0, weight: 2, names: 1 }],
};

describe("buildGraph", () => {
  test("nodes carry symbols and lang; a pair present in both directions is one undirected edge", () => {
    const g = buildGraph(payload);
    expect(g.order).toBe(3);
    expect(g.size).toBe(2);
    expect(g.getNodeAttribute("c.ts", "symbols")).toBe(9);
    expect(g.getNodeAttribute("b.ts", "lang")).toBeNull();
    expect(g.getEdgeAttribute(g.edge("a.ts", "b.ts")!, "weight")).toBeCloseTo(1.5);
    // Sum, not max: a name referenced both ways (once from each direction) is counted
    // in each direction's `names`, so the merged undirected edge's total is 1 + 2 = 3.
    expect(g.getEdgeAttribute(g.edge("a.ts", "b.ts")!, "names")).toBe(3);
  });
});

describe("layoutGraph", () => {
  test("is deterministic and places every node", () => {
    const p1 = layoutGraph(buildGraph(payload));
    const p2 = layoutGraph(buildGraph(payload));
    expect(Object.keys(p1).sort()).toEqual(["a.ts", "b.ts", "c.ts"]);
    expect(p1).toEqual(p2);
    expect(Number.isFinite(p1["a.ts"].x)).toBe(true);
  });

  test("a seed keeps surviving nodes near their old positions", () => {
    const seed = { "a.ts": { x: 100, y: 100 }, "b.ts": { x: 110, y: 100 }, "zombie.ts": { x: 0, y: 0 } };
    const p = layoutGraph(buildGraph(payload), seed);
    expect(p["zombie.ts"]).toBeUndefined();
    // Seeded nodes stay in the same neighbourhood; the unseeded node is placed too.
    expect(Math.abs(p["a.ts"].x - 100)).toBeLessThan(60);
    expect(p["c.ts"]).toBeDefined();
  });

  test("seeded_survivors_keep_their_exact_positions", () => {
    const seed = { "a.ts": { x: 100, y: 100 }, "b.ts": { x: 110, y: 100 } };
    const p = layoutGraph(buildGraph(payload), seed);
    expect(p["a.ts"]).toEqual(seed["a.ts"]);
    expect(p["b.ts"]).toEqual(seed["b.ts"]);
    expect(Number.isFinite(p["c.ts"].x)).toBe(true);
    expect(Number.isFinite(p["c.ts"].y)).toBe(true);
  });
});

describe("separateCoincident", () => {
  test("coincident_nodes_are_separated_deterministically", () => {
    const g1 = buildGraph(payload);
    applyPositions(g1, { "a.ts": { x: 5, y: 5 }, "b.ts": { x: 5, y: 5 }, "c.ts": { x: 9, y: 9 } });
    separateCoincident(g1);
    const a1 = g1.getNodeAttributes("a.ts"), b1 = g1.getNodeAttributes("b.ts");
    expect(a1.x === b1.x && a1.y === b1.y).toBe(false);

    const g2 = buildGraph(payload);
    applyPositions(g2, { "a.ts": { x: 5, y: 5 }, "b.ts": { x: 5, y: 5 }, "c.ts": { x: 9, y: 9 } });
    separateCoincident(g2);
    const a2 = g2.getNodeAttributes("a.ts"), b2 = g2.getNodeAttributes("b.ts");
    expect({ x: a1.x, y: a1.y }).toEqual({ x: a2.x, y: a2.y });
    expect({ x: b1.x, y: b1.y }).toEqual({ x: b2.x, y: b2.y });
  });
});

describe("layout cache", () => {
  beforeEach(() => localStorage.clear());
  test("round-trips per index version and tolerates garbage", () => {
    expect(loadLayout("v1")).toBeNull();
    saveLayout("v1", { "a.ts": { x: 1, y: 2 } });
    expect(loadLayout("v1")).toEqual({ "a.ts": { x: 1, y: 2 } });
    expect(loadLayout("v2")).toBeNull();
    localStorage.setItem(layoutCacheKey("v3"), "{not json");
    expect(loadLayout("v3")).toBeNull();
  });

  test("saving evicts every other cached layout so only the newest survives", () => {
    saveLayout("v1", { "a.ts": { x: 1, y: 2 } });
    saveLayout("v2", { "b.ts": { x: 3, y: 4 } });
    expect(loadLayout("v1")).toBeNull();
    expect(loadLayout("v2")).toEqual({ "b.ts": { x: 3, y: 4 } });
    // Only the layout cache is evicted, not unrelated keys.
    localStorage.setItem("singularrag.view", "map");
    saveLayout("v3", { "c.ts": { x: 5, y: 6 } });
    expect(localStorage.getItem("singularrag.view")).toBe("map");
    expect(loadLayout("v2")).toBeNull();
    expect(loadLayout("v3")).toEqual({ "c.ts": { x: 5, y: 6 } });
  });

  test("loadLayout rejects valid JSON that is the wrong shape", () => {
    localStorage.setItem(layoutCacheKey("v4"), JSON.stringify({ "a.ts": { x: 1 } }));
    expect(loadLayout("v4")).toBeNull();
    localStorage.setItem(layoutCacheKey("v5"), JSON.stringify({ "a.ts": { x: "1", y: 2 } }));
    expect(loadLayout("v5")).toBeNull();
    localStorage.setItem(layoutCacheKey("v6"), JSON.stringify({ "a.ts": { x: NaN, y: 2 } }));
    expect(loadLayout("v6")).toBeNull();
    localStorage.setItem(layoutCacheKey("v7"), JSON.stringify(["not", "an", "object"]));
    expect(loadLayout("v7")).toBeNull();
    localStorage.setItem(layoutCacheKey("v8"), JSON.stringify({ "a.ts": { x: 1, y: 2 } }));
    expect(loadLayout("v8")).toEqual({ "a.ts": { x: 1, y: 2 } });
  });
});

describe("applyPositions", () => {
  test("sets x and y on every node, origin for unknown ones", () => {
    const g = buildGraph(payload);
    applyPositions(g, { "a.ts": { x: 5, y: 6 } });
    expect(g.getNodeAttributes("a.ts")).toMatchObject({ x: 5, y: 6 });
    expect(g.getNodeAttributes("b.ts")).toMatchObject({ x: 0, y: 0 });
  });
});
