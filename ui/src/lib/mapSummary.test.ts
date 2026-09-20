import { describe, expect, test } from "bun:test";
import { summaryLabel } from "./mapSummary";

describe("summaryLabel", () => {
  test("with a retrieval", () => {
    expect(summaryLabel(486, { id: 17, served: 30, cut: 25 }, 2)).toBe("Map of 486 files. Retrieval 17: 30 served, 25 cut, 431 untouched. 2 boundaries.");
  });
  test("without a retrieval, singular boundary", () => {
    expect(summaryLabel(10, null, 1)).toBe("Map of 10 files. No retrieval selected. 1 boundary.");
  });
  test("zero boundaries", () => {
    expect(summaryLabel(0, null, 0)).toBe("Map of 0 files. No retrieval selected. 0 boundaries.");
  });
});
