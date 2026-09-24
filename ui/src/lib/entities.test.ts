import { describe, expect, test } from "bun:test";
import { hoverLines } from "./entities";

describe("hoverLines", () => {
  test("a file node hovers its label only", () => {
    expect(hoverLines({ label: "src/a.ts" })).toEqual(["src/a.ts"]);
    // A file never shows a description, even if one were present.
    expect(hoverLines({ label: "src/a.ts", kind: "file", description: "x" })).toEqual(["src/a.ts"]);
  });
  test("an entity hovers its name, then its description wrapped to a legible width", () => {
    const lines = hoverLines({ label: "Ada", kind: "entity", description: "Wrote the first published program for a general purpose computing machine designed by Babbage." });
    expect(lines[0]).toBe("Ada");
    expect(lines.length).toBeGreaterThan(2);
    for (const l of lines.slice(1)) expect(l.length).toBeLessThanOrEqual(48);
    expect(lines.slice(1).join(" ")).toBe("Wrote the first published program for a general purpose computing machine designed by Babbage.");
  });
  test("a long description is cut to four lines with an ellipsis; an unbroken word is split", () => {
    const lines = hoverLines({ label: "X", kind: "entity", description: "word ".repeat(80) });
    expect(lines).toHaveLength(5);
    expect(lines[4].endsWith("…")).toBe(true);
    expect(lines[4].length).toBeLessThanOrEqual(48);
    const long = hoverLines({ label: "X", kind: "entity", description: "a".repeat(60) });
    expect(long.slice(1).every((l) => l.length <= 48)).toBe(true);
  });
  test("an entity without a description hovers only its name; no label hovers nothing", () => {
    expect(hoverLines({ label: "Ada", kind: "entity", description: "" })).toEqual(["Ada"]);
    expect(hoverLines({ label: null })).toEqual([]);
  });
});
