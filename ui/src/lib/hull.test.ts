import { describe, expect, test } from "bun:test";
import { convexHull, padHull } from "./hull";

describe("convexHull", () => {
  test("degenerate inputs", () => {
    expect(convexHull([])).toEqual([]);
    expect(convexHull([{ x: 1, y: 1 }])).toEqual([{ x: 1, y: 1 }]);
    expect(convexHull([{ x: 0, y: 0 }, { x: 2, y: 0 }])).toEqual([{ x: 0, y: 0 }, { x: 2, y: 0 }]);
  });
  test("drops interior points and returns counter-clockwise vertices", () => {
    const h = convexHull([{ x: 0, y: 0 }, { x: 4, y: 0 }, { x: 4, y: 4 }, { x: 0, y: 4 }, { x: 2, y: 2 }, { x: 1, y: 1 }]);
    expect(h).toHaveLength(4);
    expect(h).not.toContainEqual({ x: 2, y: 2 });
    // Shoelace area positive => counter-clockwise.
    let area = 0;
    for (let i = 0; i < h.length; i++) { const a = h[i], b = h[(i + 1) % h.length]; area += a.x * b.y - b.x * a.y; }
    expect(area).toBeGreaterThan(0);
  });
});

describe("padHull", () => {
  test("one point becomes a circle, two a pill, three or more grow outward", () => {
    expect(padHull([{ x: 0, y: 0 }], 10)).toHaveLength(12);
    expect(padHull([{ x: 0, y: 0 }, { x: 10, y: 0 }], 5)).toHaveLength(16);
    const tri = padHull([{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 0, y: 10 }], 2);
    expect(tri).toHaveLength(3);
    expect(tri[0].x).toBeLessThan(0);
    expect(tri[0].y).toBeLessThan(0);
  });
});
