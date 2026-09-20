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
