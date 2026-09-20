import { describe, expect, test } from "bun:test";
import { addToBoundary, boundariesOf, boundaryNames, isExcluded, isPinned, removeFromBoundary, setNote, togglePin, toggleExclude } from "./mapEdits";
import type { MapConfig } from "@/api/types";

const empty: MapConfig = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };
const base: MapConfig = { pin: [], exclude: [], note: [], boundary: [{ name: "auth", paths: ["src/a.ts"] }], deny: { extra_patterns: [] } };

describe("map edits", () => {
  test("pin toggles at file and symbol level", () => {
    let c = togglePin(empty, "src/a.ts");
    expect(c.pin).toEqual([{ path: "src/a.ts" }]);
    expect(isPinned(c, "src/a.ts")).toBe(true);
    c = togglePin(c, "src/a.ts", "f");
    expect(c.pin).toEqual([{ path: "src/a.ts" }, { path: "src/a.ts", symbol: "f" }]);
    c = togglePin(c, "src/a.ts");
    expect(c.pin).toEqual([{ path: "src/a.ts", symbol: "f" }]);
  });
  test("exclude toggles and does not mutate the input", () => {
    const c = toggleExclude(empty, "src/legacy/");
    expect(c.exclude).toEqual([{ path: "src/legacy/" }]);
    expect(isExcluded(c, "src/legacy/")).toBe(true);
    expect(empty.exclude).toEqual([]);
    expect(toggleExclude(c, "src/legacy/").exclude).toEqual([]);
  });
  test("setNote replaces, and an empty text removes", () => {
    let c = setNote(empty, "src/a.ts", undefined, "hello");
    expect(c.note).toEqual([{ path: "src/a.ts", text: "hello" }]);
    c = setNote(c, "src/a.ts", undefined, "again");
    expect(c.note).toEqual([{ path: "src/a.ts", text: "again" }]);
    c = setNote(c, "src/a.ts", undefined, "");
    expect(c.note).toEqual([]);
  });
});

describe("boundaries", () => {
  test("add creates or extends, trims, ignores empty and duplicates", () => {
    expect(addToBoundary(base, " http ", "src/h.ts").boundary).toEqual([{ name: "auth", paths: ["src/a.ts"] }, { name: "http", paths: ["src/h.ts"] }]);
    expect(addToBoundary(base, "auth", "src/b.ts").boundary[0].paths).toEqual(["src/a.ts", "src/b.ts"]);
    expect(addToBoundary(base, "auth", "src/a.ts")).toEqual(base);
    expect(addToBoundary(base, "   ", "src/a.ts")).toEqual(base);
  });
  test("remove drops the boundary when empty and is a no-op otherwise", () => {
    expect(removeFromBoundary(base, "auth", "src/a.ts").boundary).toEqual([]);
    const two = addToBoundary(base, "auth", "src/b.ts");
    expect(removeFromBoundary(two, "auth", "src/a.ts").boundary).toEqual([{ name: "auth", paths: ["src/b.ts"] }]);
    expect(removeFromBoundary(base, "nope", "src/a.ts")).toEqual(base);
  });
  test("boundariesOf and boundaryNames", () => {
    expect(boundariesOf(base, "src/a.ts")).toEqual(["auth"]);
    expect(boundariesOf(base, "src/z.ts")).toEqual([]);
    expect(boundaryNames(addToBoundary(base, "http", "src/h.ts"))).toEqual(["auth", "http"]);
  });
});
