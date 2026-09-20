import { describe, expect, test } from "bun:test";
import { isExcluded, isPinned, setNote, togglePin, toggleExclude } from "./mapEdits";
import type { MapConfig } from "@/api/types";

const empty: MapConfig = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };

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
