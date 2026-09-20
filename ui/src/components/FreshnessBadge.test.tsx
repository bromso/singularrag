import { describe, expect, test } from "bun:test";
import type { Status } from "@/api/types";
import { freshnessText } from "./FreshnessBadge";

const base: Status = {
  index_version: "v1", git_head: "abc123", indexed_at_ms: 1000,
  stale_count: 0, lock_timeout: false, foreign_indexing: false, indexing: false,
  files: { indexed: 10, skipped: 2 },
  drain: { chunks: 0, last: { scanned: 0, indexed: 0, unchanged: 0, skipped: 0, removed: 0, remaining: 0, lock_timeout: false } },
};

describe("freshnessText", () => {
  test("has four states in priority order, and 'N stale' is reachable", () => {
    expect(freshnessText(null)).toBe("loading");
    expect(freshnessText(base)).toBe("fresh");
    expect(freshnessText({ ...base, stale_count: 3 })).toBe("3 stale");
    expect(freshnessText({ ...base, indexing: true, stale_count: 3 })).toBe("indexing");
    expect(freshnessText({ ...base, foreign_indexing: true, indexing: true, stale_count: 3 })).toBe("another process indexing");
  });

  test("ignores drain, which is always 0 from serve", () => {
    const draining = { ...base, drain: { chunks: 4, last: { ...base.drain.last, remaining: 9 } } };
    expect(freshnessText(draining)).toBe("fresh");
  });
});
