import { describe, expect, test } from "bun:test";
import type { Status } from "@/api/types";
import { freshnessText } from "./FreshnessBadge";

const base: Status = {
  index_version: "v1", git_head: "abc123", indexed_at_ms: 1000,
  stale_count: 0, lock_timeout: false, foreign_indexing: false, indexing: false,
  files: { indexed: 10, skipped: 2 },
};

describe("freshnessText", () => {
  test("has four states in priority order, and 'N stale' is reachable", () => {
    expect(freshnessText(null)).toBe("loading");
    expect(freshnessText(base)).toBe("fresh");
    expect(freshnessText({ ...base, stale_count: 3 })).toBe("3 stale");
    expect(freshnessText({ ...base, indexing: true, stale_count: 3 })).toBe("indexing");
    expect(freshnessText({ ...base, foreign_indexing: true, indexing: true, stale_count: 3 })).toBe("another process indexing");
  });
});
