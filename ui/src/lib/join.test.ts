import { describe, expect, test } from "bun:test";
import { joinRetrieval } from "./join";
import type { Item, TreeFile } from "@/api/types";

const files: TreeFile[] = [
  { path: "src/a.ts", lang: "typescript", skipped_reason: null, symbols: [
    { id: 1, name: "f", kind: "function", line_start: 1, line_end: 3, signature: "export function f()" },
    { id: 2, name: "g", kind: "function", line_start: 5, line_end: 7, signature: "export function g()" },
  ] },
  { path: "src/b.ts", lang: "typescript", skipped_reason: null, symbols: [
    { id: 3, name: "h", kind: "function", line_start: 1, line_end: 2, signature: "export function h()" },
  ] },
];
const r = { score: 0, file_rank: 0, seeds: [], referenced_by: [], pinned: false, fts_hit: false, query_ident_match: false };
const items: Item[] = [
  { rank: 1, symbol_id: 1, path: "src/a.ts", name: "f", line_start: 1, score: 0.5, served: true, reasons: r },
  { rank: 2, symbol_id: 2, path: "src/a.ts", name: "g", line_start: 5, score: 0.1, served: false, reasons: r },
];

describe("joinRetrieval", () => {
  test("without a retrieval every symbol is untouched and nothing is expanded", () => {
    const rows = joinRetrieval(files, null);
    expect(rows.map((f) => f.path)).toEqual(["src/a.ts", "src/b.ts"]);
    expect(rows[0].symbols.every((s) => s.status === "untouched")).toBe(true);
    expect(rows.every((f) => !f.expandedByDefault)).toBe(true);
  });
  test("with a retrieval, items join by path+name+line and touched files come first, expanded", () => {
    const rows = joinRetrieval(files, items);
    expect(rows[0].path).toBe("src/a.ts");
    expect(rows[0].served).toBe(1);
    expect(rows[0].cut).toBe(1);
    expect(rows[0].expandedByDefault).toBe(true);
    expect(rows[0].symbols.map((s) => s.status)).toEqual(["served", "cut"]);
    expect(rows[1].expandedByDefault).toBe(false);
    expect(rows[1].symbols[0].status).toBe("untouched");
  });
});

describe("join tiers", () => {
  const item = (o: Partial<Item>): Item => ({ rank: 1, symbol_id: 0, path: "src/a.ts", name: "f", line_start: 1, score: 0.5, served: true, reasons: r, ...o });

  test("tier 1: symbol_id wins even when the line differs, if path and name agree", () => {
    const rows = joinRetrieval(files, [item({ symbol_id: 2, name: "g", line_start: 99 })]);
    const g = rows[0].symbols.find((s) => s.symbol.name === "g")!;
    expect(g.status).toBe("served");
    expect(g.moved).toBe(false);
  });

  test("a reused id that points at a different symbol does not match", () => {
    const rows = joinRetrieval(files, [item({ symbol_id: 3, path: "src/a.ts", name: "f", line_start: 1 })]);
    const f = rows.find((x) => x.path === "src/a.ts")!.symbols.find((s) => s.symbol.name === "f")!;
    expect(f.status).toBe("served");
    expect(f.moved).toBe(false);
    const h = rows.find((x) => x.path === "src/b.ts")!.symbols[0];
    expect(h.status).toBe("untouched");
  });

  test("tier 2: a new id with the same path, name and line joins without moved", () => {
    const rows = joinRetrieval(files, [item({ symbol_id: 777, name: "f", line_start: 1 })]);
    const f = rows[0].symbols.find((s) => s.symbol.name === "f")!;
    expect(f.status).toBe("served");
    expect(f.moved).toBe(false);
  });

  test("tier 3: same path and name at another line joins as moved, nearest line wins", () => {
    const twoF: TreeFile[] = [{ path: "src/a.ts", lang: null, skipped_reason: null, symbols: [
      { id: 10, name: "f", kind: "function", line_start: 3, line_end: 4, signature: "f" },
      { id: 11, name: "f", kind: "function", line_start: 40, line_end: 41, signature: "f" },
    ] }];
    const rows = joinRetrieval(twoF, [item({ symbol_id: 777, name: "f", line_start: 8 })]);
    const [near, far] = rows[0].symbols;
    expect(near.status).toBe("served");
    expect(near.moved).toBe(true);
    expect(far.status).toBe("untouched");
  });

  test("tier 3 tie keeps the earlier symbol: array order is file order", () => {
    const twoF: TreeFile[] = [{ path: "src/a.ts", lang: null, skipped_reason: null, symbols: [
      { id: 20, name: "f", kind: "function", line_start: 5, line_end: 6, signature: "f" },
      { id: 21, name: "f", kind: "function", line_start: 15, line_end: 16, signature: "f" },
    ] }];
    const rows = joinRetrieval(twoF, [item({ symbol_id: 777, name: "f", line_start: 10 })]);
    const [early, late] = rows[0].symbols;
    expect(early.status).toBe("served");
    expect(early.moved).toBe(true);
    expect(late.status).toBe("untouched");
  });

  test("a name that no longer exists in the file stays unjoined", () => {
    const rows = joinRetrieval(files, [item({ symbol_id: 777, name: "gone", line_start: 1 })]);
    expect(rows[0].symbols.every((s) => s.status === "untouched")).toBe(true);
    expect(rows[0].served).toBe(0);
  });

  test("each symbol takes at most one item, by rank", () => {
    const rows = joinRetrieval(files, [item({ rank: 2, symbol_id: 777, name: "f", line_start: 1, served: false }), item({ rank: 1, symbol_id: 1, name: "f", line_start: 1, served: true })]);
    const f = rows[0].symbols.find((s) => s.symbol.name === "f")!;
    expect(f.item?.rank).toBe(1);
    expect(f.status).toBe("served");
  });
});
