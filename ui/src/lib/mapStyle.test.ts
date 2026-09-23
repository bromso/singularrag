import { describe, expect, test } from "bun:test";
import { edgeStyle, nodeSize, nodeStyle, readPalette, type EdgeCtx, type NodeCtx, type Palette } from "./mapStyle";

const SIGMA_COLOR = /^#[0-9a-fA-F]{6}$|^rgba?\(/;

const pal: Palette = { served: "#0a0", cut: "#a60", untouched: "#888", focus: "#00f", edge: "#666", edgeDim: "#ddd", label: "#000", background: "#fff", docs: "#70c", config: "#0aa", styles: "#c07" };
const base: NodeCtx = { status: null, focused: false, blastDepth: null, blastActive: false, symbols: 4, hovered: false, zoomRatio: 1, group: "code" };

describe("nodeStyle", () => {
  test("served is filled with a plain label", () => {
    const s = nodeStyle("src/a.ts", { ...base, status: "served" }, pal);
    expect(s).toMatchObject({ type: "circle", color: pal.served, label: "src/a.ts" });
  });
  test("cut is a ring whose label says cut", () => {
    const s = nodeStyle("src/a.ts", { ...base, status: "cut" }, pal);
    expect(s.type).toBe("border");
    expect(s.color).toBe(pal.background);
    expect(s.borderColor).toBe(pal.cut);
    expect(s.label).toBe("src/a.ts · cut");
  });
  test("untouched is small, grey and unlabelled unless hovered or zoomed in", () => {
    const s = nodeStyle("src/a.ts", { ...base, status: "untouched" }, pal);
    expect(s.color).toBe(pal.untouched);
    expect(s.label).toBeNull();
    expect(s.size).toBeLessThan(nodeStyle("src/a.ts", { ...base, status: "served" }, pal).size);
    expect(nodeStyle("src/a.ts", { ...base, status: "untouched", hovered: true }, pal).label).toBe("src/a.ts");
    expect(nodeStyle("src/a.ts", { ...base, status: "untouched", zoomRatio: 0.3 }, pal).label).toBe("src/a.ts");
  });
  test("untouched document nodes use the docs colour", () => {
    const s = nodeStyle("docs/a.md", { ...base, status: "untouched", group: "docs" }, pal);
    expect(s.color).toBe(pal.docs);
  });
  test("no retrieval means neutral nodes with labels", () => {
    expect(nodeStyle("src/a.ts", base, pal)).toMatchObject({ type: "circle", label: "src/a.ts" });
  });
  test("focus adds a ring on top of any status and raises zIndex", () => {
    const s = nodeStyle("src/a.ts", { ...base, status: "served", focused: true }, pal);
    expect(s.type).toBe("border");
    expect(s.borderColor).toBe(pal.focus);
    expect(s.color).toBe(pal.served);
    expect(s.zIndex).toBeGreaterThan(nodeStyle("src/a.ts", { ...base, status: "served" }, pal).zIndex);
  });
  test("blast mode dims everything outside the radius and labels depth", () => {
    const inside = nodeStyle("src/a.ts", { ...base, status: "served", blastActive: true, blastDepth: 2 }, pal);
    expect(inside.label).toBe("src/a.ts · depth 2");
    expect(inside.type).toBe("border");
    const outside = nodeStyle("src/b.ts", { ...base, status: "served", blastActive: true, blastDepth: null }, pal);
    expect(outside.color).toBe(pal.untouched);
    expect(outside.label).toBeNull();
  });
  test("nodeSize grows with symbols and caps", () => {
    expect(nodeSize(0)).toBe(4);
    expect(nodeSize(100)).toBe(14);
    expect(nodeSize(9)).toBeCloseTo(8.5);
  });
});

describe("edgeStyle", () => {
  test("edges of the focused file are full, others dim; blast mode hides edges outside the radius", () => {
    expect(edgeStyle({ touchesFocused: true, blastActive: false, touchesBlast: false }, pal)).toMatchObject({ color: pal.edge, hidden: false });
    expect(edgeStyle({ touchesFocused: false, blastActive: false, touchesBlast: false }, pal)).toMatchObject({ color: pal.edgeDim, hidden: false });
    expect(edgeStyle({ touchesFocused: false, blastActive: true, touchesBlast: false }, pal).hidden).toBe(true);
    expect(edgeStyle({ touchesFocused: false, blastActive: true, touchesBlast: true }, pal).hidden).toBe(false);
  });
});

describe("readPalette", () => {
  test("falls back to defaults when the variables are unset", () => {
    const el = document.createElement("div");
    document.body.appendChild(el);
    const p = readPalette(el);
    for (const v of Object.values(p)) expect(v).toMatch(SIGMA_COLOR);
  });
});

describe("sigma-parsable colours", () => {
  test("every colour the reducers emit is sigma-parsable", () => {
    const pal6: Palette = { served: "#047857", cut: "#b45309", untouched: "#9ca3af", focus: "#2563eb", edge: "#6b7280", edgeDim: "#e5e7eb", label: "#111827", background: "#ffffff", docs: "#7c3aed", config: "#0e7490", styles: "#be185d" };
    const statuses: (NodeCtx["status"])[] = [null, "served", "cut", "untouched"];
    const bools = [false, true];
    const collected: string[] = [];
    for (const status of statuses) {
      for (const focused of bools) {
        for (const blastActive of bools) {
          for (const blastDepth of [null, 0, 2]) {
            const s = nodeStyle("src/a.ts", { status, focused, blastDepth, blastActive, symbols: 4, hovered: false, zoomRatio: 1, group: "code" }, pal6);
            collected.push(s.color);
            if (s.borderColor) collected.push(s.borderColor);
          }
        }
      }
    }
    const edgeCtxs: EdgeCtx[] = [
      { touchesFocused: true, blastActive: false, touchesBlast: false },
      { touchesFocused: false, blastActive: false, touchesBlast: false },
      { touchesFocused: false, blastActive: true, touchesBlast: false },
      { touchesFocused: false, blastActive: true, touchesBlast: true },
    ];
    for (const ctx of edgeCtxs) collected.push(edgeStyle(ctx, pal6).color);
    expect(collected.length).toBeGreaterThan(0);
    for (const c of collected) expect(c).toMatch(SIGMA_COLOR);
  });
});
