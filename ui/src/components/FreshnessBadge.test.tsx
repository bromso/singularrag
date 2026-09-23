import { afterEach, describe, expect, test } from "bun:test";
import { cleanup, render, screen } from "@testing-library/react";
import type { Status } from "@/api/types";
import { FreshnessBadge, freshnessText } from "./FreshnessBadge";

afterEach(cleanup);

const base: Status = {
  index_version: "v1", git_head: "abc123", indexed_at_ms: 1000,
  stale_count: 0, lock_timeout: false, foreign_indexing: false, indexing: false,
  files: { indexed: 10, skipped: 2 },
  roots: [{ name: "", path: "/repo", git_head: "abc123" }],
  entities_pending: 0, models_unavailable: false, embeddings_rebuilding: false,
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

describe("FreshnessBadge head", () => {
  test("a composed workspace head renders whole; a plain sha renders seven characters", () => {
    render(<FreshnessBadge status={{ ...base, git_head: "app:9b1e0d4 notes:none" }} />);
    expect(screen.getByText("app:9b1e0d4 notes:none")).toBeTruthy();
    cleanup();
    render(<FreshnessBadge status={{ ...base, git_head: "9b1e0d4c0ffee" }} />);
    expect(screen.getByText("9b1e0d4")).toBeTruthy();
  });
});
